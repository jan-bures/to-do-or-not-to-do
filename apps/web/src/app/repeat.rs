//! Repeat presets and the custom draft for the detail pane.

use super::{Detail, ctx, icon, model, pickers::DetailPicker, platform};
use leptos::prelude::*;
use todo_client::intent::{
    quick_add,
    repeat::{self, Preset, describe_repeat},
};
use todo_core::{
    ids::TaskId,
    model::Task,
    recurrence::{MAX_INTERVAL, Recurrence, RecurrenceError, Rule},
    time::{Date, Weekday},
};
use todo_design::Icon;

const PRESETS: [(Preset, &str, &str); 5] = [
    (Preset::Daily, "Daily", "repeat-daily"),
    (Preset::Weekdays, "Weekdays", "repeat-weekdays"),
    (Preset::Weekly, "Weekly", "repeat-weekly"),
    (Preset::Monthly, "Monthly", "repeat-monthly"),
    (Preset::Yearly, "Yearly", "repeat-yearly"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Clone, Copy)]
struct Custom {
    interval: u32,
    unit: Unit,
    days: [bool; 7],
    month_day: u8,
}

impl Custom {
    fn new(task: &Task, today: Date) -> Self {
        let anchor = task.due.unwrap_or(today);
        let mut draft = Self {
            interval: 1,
            unit: Unit::Day,
            days: Weekday::ALL.map(|d| d == Weekday::of(anchor)),
            month_day: u8::try_from(anchor.day()).unwrap_or(1),
        };
        if let Some(r) = &task.recurrence {
            (draft.unit, draft.interval) = match &r.rule {
                Rule::Daily { interval } => (Unit::Day, *interval),
                Rule::Weekly { interval, days } => {
                    draft.days = Weekday::ALL.map(|d| days.contains(&d));
                    (Unit::Week, *interval)
                }
                Rule::Monthly { interval, day } => {
                    draft.month_day = *day;
                    (Unit::Month, *interval)
                }
                Rule::Yearly { interval } => (Unit::Year, *interval),
            };
        }
        draft
    }

    fn rule(self) -> Rule {
        let interval = self.interval;
        match self.unit {
            Unit::Day => Rule::Daily { interval },
            Unit::Week => Rule::Weekly {
                interval,
                days: Weekday::ALL
                    .into_iter()
                    .filter(|d| self.days[d.offset()])
                    .collect(),
            },
            Unit::Month => Rule::Monthly {
                interval,
                day: self.month_day,
            },
            Unit::Year => Rule::Yearly { interval },
        }
    }
}

fn error_text(e: RecurrenceError) -> String {
    match e {
        RecurrenceError::NoWeekdays => "Choose at least one weekday.".into(),
        RecurrenceError::NoOccurrence => "No future date matches this rule.".into(),
        _ => format!("Choose an interval from 1 to {MAX_INTERVAL}."),
    }
}

#[component]
pub(super) fn RepeatEditor(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let picker = DetailPicker::new(c, id, "edit-task-repeat", "repeat-picker");
    let custom = RwSignal::new(None::<Custom>);
    let error = RwSignal::new(None::<String>);
    let editing_custom = Memo::new(move |_| custom.with(Option::is_some));
    let dates = Memo::new(move |_| {
        c.state
            .with(|s| s.tasks.get(&id).map(|t| (t.due, t.recurrence.clone())))
    });
    let now = move || {
        if c.live {
            platform::now()
        } else {
            c.now.get_untracked()
        }
    };
    let change = move |preset: Option<Preset>, draft: Option<Custom>| {
        if c.read_only() {
            return;
        }
        let today = now().today();
        let result = c.state.with_untracked(|s| {
            let Some(t) = s.tasks.get(&id) else {
                return Ok(None);
            };
            let rule = draft.map(Custom::rule).or_else(|| {
                preset.map(|p| p.rule_for(t.due.unwrap_or(today), t.recurrence.as_ref()))
            });
            repeat::set_repeat(s, id, rule, today)
        });
        match result {
            Ok(cmd) => {
                let label = if preset.is_none() && draft.is_none() {
                    "Repeat cleared."
                } else {
                    "Repeat changed."
                };
                if cmd.is_none_or(|cmd| c.submit_undoable(cmd, label.into(), None)) {
                    picker.close(true);
                } else {
                    error.set(c.toast.get_untracked().map(|t| t.text));
                }
            }
            Err(e) => error.set(Some(error_text(e))),
        }
    };
    let selected = move |p: Preset| {
        dates.with(|d| {
            d.as_ref().is_some_and(|(due, rule)| {
                rule.as_ref().is_some_and(|r| {
                    r.rule == p.rule_for(due.unwrap_or_else(|| c.now.get().today()), Some(r))
                })
            })
        })
    };
    let missing_due = move || dates.with(|d| d.as_ref().is_some_and(|(due, _)| due.is_none()));
    view! {
        <div class="prop">
            <dt>{icon(Icon::Repeat)}"Repeat"</dt>
            <dd>
                <button type="button" id=picker.anchor class="prop-edit" aria-label="Repeat"
                    aria-haspopup="dialog" aria-controls=picker.id aria-describedby="repeat-description"
                    aria-expanded=move || picker.open.get().to_string()
                    popovertarget=picker.id title="Change repeat (R)" aria-keyshortcuts="R" disabled=move || c.read_only()
                    on:click=move |ev| {
                        ev.prevent_default();
                        if picker.open.get_untracked() { picker.close(true); }
                        else { custom.set(None); error.set(None); picker.show("repeat-daily"); }
                    }><span id="repeat-description">{move || detail.with(|d| d.as_ref().map(|d| d.repeat.clone()))}</span></button>
                <div class="room-action-menu detail-picker" role="dialog" aria-labelledby="repeat-picker-heading"
                    id=picker.id popover="auto" node_ref=picker.node on:toggle=move |_| picker.toggle_event()
                    on:keydown=move |ev| picker.key(c, (!editing_custom.get_untracked()).then_some(1), &ev)>
                    <Show when=move || picker.open.get()>
                        <Show when=move || editing_custom.get() fallback=move || view! {
                            <div class="picker-page">
                                <h3 id="repeat-picker-heading">"Repeat"</h3>
                                {PRESETS.into_iter().map(|(p, name, key)| view! {
                                    <button type="button" id=key data-picker-option="" aria-pressed=move || selected(p).to_string()
                                        on:click=move |_| change(Some(p), None)>
                                        <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
                                        {move || dates.with(|d| {
                                            let anchor = d.as_ref().and_then(|(due, _)| *due).unwrap_or_else(|| c.now.get().today());
                                            match p {
                                                Preset::Weekly => format!("Weekly on {}", model::core_weekday(Weekday::of(anchor))),
                                                Preset::Monthly => match p.rule_for(anchor, d.as_ref().and_then(|(_, r)| r.as_ref())) {
                                                    Rule::Monthly { day, .. } => format!("Monthly on day {day}"),
                                                    _ => name.into(),
                                                },
                                                _ => name.into(),
                                            }
                                        })}
                                    </button>
                                }).collect_view()}
                                <button type="button" data-picker-option="" aria-pressed=move || dates.with(|d| {
                                        d.as_ref().is_some_and(|(_, r)| r.is_some()) && !PRESETS.iter().any(|(p, _, _)| selected(*p))
                                    }).to_string()
                                    on:click=move |_| {
                                        custom.set(c.state.with_untracked(|s| s.tasks.get(&id).map(|t| Custom::new(t, now().today()))));
                                        error.set(None);
                                        picker.focus_page("repeat-interval");
                                    }><span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>"Custom…"</button>
                                <button type="button" data-picker-option="" aria-pressed=move || dates.with(|d| d.as_ref().is_some_and(|(_, r)| r.is_none())).to_string()
                                    on:click=move |_| change(None, None)>
                                    <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>"Don’t repeat"
                                </button>
                                <Show when=missing_due><p class="picker-note">"Sets the first matching due date."</p></Show>
                            </div>
                        }>
                            <CustomEditor custom=custom dates=dates today=Signal::derive(move || c.now.get().today())
                                apply=Callback::new(move |draft| change(None, Some(draft)))
                                back=Callback::new(move |()| {
                                    custom.set(None);
                                    error.set(None);
                                    picker.focus_page("repeat-daily");
                                })/>
                        </Show>
                        {move || error.get().map(|text| view! { <p class="prop-error" role="alert">{text}</p> })}
                    </Show>
                </div>
            </dd>
        </div>
    }
}

type Dates = Memo<Option<(Option<Date>, Option<Recurrence>)>>;

#[component]
fn CustomEditor(
    custom: RwSignal<Option<Custom>>,
    dates: Dates,
    today: Signal<Date>,
    apply: Callback<Custom>,
    back: Callback<()>,
) -> impl IntoView {
    let summary = Memo::new(move |_| {
        let draft = custom.get()?;
        let (due, repeat) = dates.get()?;
        let repeat = Recurrence {
            rule: draft.rule(),
            until: repeat.and_then(|r| r.until),
        };
        Some(
            if due.is_none() {
                repeat.first_due(today.get()).map(|day| {
                    format!(
                        "{}. Due will be {}.",
                        describe_repeat(&repeat),
                        quick_add::long_date(day)
                    )
                })
            } else {
                repeat.validate().map(|()| describe_repeat(&repeat))
            }
            .map_err(error_text),
        )
    });
    let interval = custom.with_untracked(|d| d.map_or(1, |d| d.interval));
    view! {
        <form class="picker-page" on:submit=move |ev| {
            ev.prevent_default();
            if summary.with_untracked(|s| s.as_ref().is_some_and(Result::is_ok))
                && let Some(draft) = custom.get_untracked() { apply.run(draft); }
        }>
            <h3 id="repeat-picker-heading">"Custom repeat"</h3>
            <label class="picker-field" for="repeat-interval">"Interval"
                <input id="repeat-interval" class="room-name-input" type="number" inputmode="numeric" min="1" max=MAX_INTERVAL step="1" required
                    value=interval on:input=move |ev| {
                        let n = event_target_value(&ev).parse().unwrap_or_default();
                        custom.update(|v| { if let Some(v) = v { v.interval = n; } });
                    }/>
            </label>
            <label class="picker-field" for="repeat-unit">"Unit"
                <select class="room-name-input" id="repeat-unit" on:change=move |ev| {
                    let unit = match event_target_value(&ev).as_str() {
                        "Week" => Unit::Week, "Month" => Unit::Month, "Year" => Unit::Year, _ => Unit::Day,
                    };
                    custom.update(|v| { if let Some(v) = v { v.unit = unit; } });
                }>{[("Day", Unit::Day), ("Week", Unit::Week), ("Month", Unit::Month), ("Year", Unit::Year)].into_iter().map(|(name, unit)| view! {
                    <option value=name prop:selected=move || custom.with(|v| v.is_some_and(|v| v.unit == unit))>{name}</option>
                }).collect_view()}</select>
            </label>
            <Show when=move || custom.with(|v| v.is_some_and(|v| v.unit == Unit::Week))>
                <fieldset class="repeat-days"><legend>"On weekdays"</legend>
                    {Weekday::ALL.into_iter().zip(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]).map(|(day, label)| view! {
                        <button type="button" title=model::core_weekday(day) aria-label=model::core_weekday(day)
                            aria-pressed=move || custom.with(|v| v.is_some_and(|v| v.days[day.offset()])).to_string()
                            on:click=move |_| custom.update(|v| { if let Some(v) = v { v.days[day.offset()] = !v.days[day.offset()]; } })>{label}</button>
                    }).collect_view()}
                </fieldset>
            </Show>
            <p class="picker-note" role="status">{move || summary.get().map(|r| r.unwrap_or_else(|e| e))}</p>
            <div class="picker-buttons">
                <button type="button" on:click=move |_| back.run(())>"Back"</button>
                <button type="submit" class="primary" disabled=move || !summary.with(|s| s.as_ref().is_some_and(Result::is_ok))>"Apply repeat"</button>
            </div>
        </form>
    }
}
