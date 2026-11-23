//! The room composer and its two anchored pickers.

use leptos::prelude::*;
use todo_client::intent::{self, calendar, quick_add};
use todo_core::{model::Priority, time::Date};
use todo_design::Icon;

use super::pickers::{DetailPicker, choice_focus};
use super::{backend, ctx, icon, platform};

const INPUT: &str = "qa-input";
const ACTIVE_DAY: &str = "qa-active-day";

#[component]
pub(super) fn QuickAdd() -> impl IntoView {
    let c = ctx();
    let draft = RwSignal::new(quick_add::Draft::default());
    let date_picker = DetailPicker::anchored("qa-date", "qa-calendar");
    let priority_picker = DetailPicker::anchored("qa-priority", "qa-priority-menu");
    let month = RwSignal::<Option<calendar::Month>>::new(None);
    let opened_today = RwSignal::<Option<Date>>::new(None);
    let add_placeholder = Memo::new(move |_| c.view_room.with(|r| r.add_placeholder.clone()));
    let room = Memo::new(move |_| c.view_room.with(|r| r.id.clone()));
    Effect::new(move |_| {
        room.track();
        if c.read_only() || date_picker.open.get_untracked() {
            date_picker.close(false);
        }
        if c.read_only() || priority_picker.open.get_untracked() {
            priority_picker.close(false);
        }
    });
    let now = move || {
        if c.live {
            let now = platform::now();
            c.now.set(now);
            now
        } else {
            c.now.get_untracked()
        }
    };
    let add = move || {
        if c.read_only() || add_placeholder.with_untracked(Option::is_none) {
            return;
        }
        let eval = now().eval();
        let (id, commands) = c.state.with_untracked(|state| {
            draft.with_untracked(|draft| {
                intent::submit_draft(
                    state,
                    &room.get_untracked(),
                    draft,
                    &eval,
                    backend::new_uuid,
                )
            })
        });
        if id.is_some() && commands.into_iter().all(|command| c.submit(command)) {
            draft.set(quick_add::Draft::default());
        }
    };
    let parsed = Memo::new(move |_| draft.with(|draft| draft.parsed(&c.now.get().eval())));
    let due = Memo::new(move |_| parsed.with(|p| p.due));
    let priority = Memo::new(move |_| parsed.with(|p| p.priority.unwrap_or_default()));
    let date = Memo::new(move |_| {
        due.get()
            .map(|d| quick_add::short_date(d, c.now.get().eval().today))
    });
    let chip_label = Memo::new(move |prev: Option<&String>| {
        date.get().or_else(|| prev.cloned()).unwrap_or_default()
    });
    let choose_due = move |day| {
        if !c.read_only() {
            draft.update(|d| d.pick_due(day, &now().eval()));
        }
        date_picker.close(false);
        platform::focus(INPUT);
    };
    let choose_priority = move |level| {
        if !c.read_only() {
            draft.update(|d| d.pick_priority(level, &now().eval()));
        }
        priority_picker.close(false);
        platform::focus(INPUT);
    };
    let show_date = move || {
        if date_picker.open.get_untracked() {
            date_picker.close(true);
        } else if !c.read_only() {
            let eval = now().eval();
            opened_today.set(Some(eval.today));
            let day = draft
                .with_untracked(|d| d.parsed(&eval).due)
                .unwrap_or(eval.today);
            month.set(Some(calendar::Month::new(day)));
            date_picker.show(ACTIVE_DAY);
        }
    };
    let show_priority = move || {
        if priority_picker.open.get_untracked() {
            priority_picker.close(true);
        } else if !c.read_only() {
            priority_picker.show_choice(priority_picker.id);
        }
    };
    view! {
        <Show when=move || add_placeholder.with(Option::is_some)>
            <form class="quick-add" on:submit=move |ev| { ev.prevent_default(); add(); }>
                <span class="qa-plus">{icon(Icon::Plus)}</span>
                <input node_ref=c.quick_add id=INPUT class="qa-input" type="text"
                    disabled=move || c.read_only() autocomplete="off" enterkeyhint="done" aria-label="Add a task"
                    placeholder=move || add_placeholder.get().unwrap_or_default()
                    prop:value=move || draft.with(|d| d.text.clone())
                    on:input=move |ev| draft.update(|d| d.edit(event_target_value(&ev), &c.now.get_untracked().eval())) />
                <span class="qa-date-announcement" role="status">{move || {
                    let mut labels = Vec::new();
                    if let Some(date) = date.get() { labels.push(format!("Due {date}")); }
                    if priority.get() != Priority::None {
                        labels.push(format!("{} priority", quick_add::priority_option(priority.get()).label));
                    }
                    labels.join(". ")
                }}</span>
                <div class="qa-chips" class:has-chips=move || due.get().is_some() || priority.get() != Priority::None>
                    <button type="button" class="qa-date-chip" class:no-date=move || due.get().is_none()
                        disabled=move || due.get().is_none() || c.read_only()
                        aria-label=move || format!("Remove due date: {}", chip_label.get())
                        on:click=move |_| choose_due(None)>
                        {icon(Icon::Calendar)}<span>{move || chip_label.get()}</span>{icon(Icon::X)}
                    </button>
                    <Show when=move || priority.get() != Priority::None>
                        <button type="button" class="qa-priority-chip" disabled=move || c.read_only()
                            aria-label=move || format!("Remove priority: {}", quick_add::priority_option(priority.get()).label)
                            on:click=move |_| choose_priority(Priority::None)>
                            {move || quick_add::priority_option(priority.get()).mark}{icon(Icon::X)}
                        </button>
                    </Show>
                </div>
                <div class="qa-tools">
                    <button type="button" id=date_picker.anchor class="qa-tool" class:has-value=move || due.get().is_some()
                        title="Choose due date" aria-label="Choose due date" aria-haspopup="dialog" aria-controls=date_picker.id
                        aria-expanded=move || date_picker.open.get().to_string() disabled=move || c.read_only()
                        on:click=move |_| show_date()>{icon(Icon::Calendar)}</button>
                    <button type="button" id=priority_picker.anchor class="qa-tool" class:has-value=move || priority.get() != Priority::None
                        title="Choose priority" aria-label="Choose priority" aria-haspopup="listbox" aria-controls=priority_picker.id
                        aria-expanded=move || priority_picker.open.get().to_string() disabled=move || c.read_only()
                        on:click=move |_| show_priority()>{icon(Icon::Flag)}</button>
                </div>
                <span class="qa-hints" aria-hidden="true" hidden=move || c.read_only()>
                    <span class="keycap">"N"</span>
                </span>
                <div class="room-action-menu qa-calendar" popover="auto" id=date_picker.id node_ref=date_picker.node
                    role="dialog" aria-label="Choose due date" on:toggle=move |_| date_picker.toggle_event()
                    on:keydown=move |ev| date_picker.key(c, None, &ev)>
                    <Calendar month=month today=opened_today due=due choose=Callback::new(choose_due) />
                </div>
                <div class="room-action-menu qa-priority-menu" popover="auto" id=priority_picker.id node_ref=priority_picker.node
                    role="listbox" aria-label="Choose priority" on:toggle=move |_| priority_picker.toggle_event()
                    on:focusin=choice_focus on:keydown=move |ev| priority_picker.key(c, Some(1), &ev)>
                    {quick_add::PRIORITIES.into_iter().map(|option| view! {
                        <button type="button" role="option" tabindex="-1" data-picker-option="" class="room-action-item"
                            aria-selected=move || (priority.get() == option.level).to_string()
                            on:click=move |_| choose_priority(option.level)>
                            {icon(Icon::Flag)}<span>{option.label}</span>
                            <span class="mono" aria-hidden="true">{option.mark}</span>
                            <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
                        </button>
                    }).collect_view()}
                </div>
            </form>
        </Show>
    }
}

#[component]
fn Calendar(
    month: RwSignal<Option<calendar::Month>>,
    today: RwSignal<Option<Date>>,
    due: Memo<Option<Date>>,
    choose: Callback<Option<Date>>,
) -> impl IntoView {
    // Moving within the month changes only the roving tab stop, not the DOM rows.
    let days = Memo::new(move |_| {
        month.get().map(|mut month| {
            month.cursor = month.cursor.first_of_month();
            month
        })
    });
    let step = move |delta| {
        month.update(|value| {
            if let Some(next) = value.and_then(|m| m.shifted(delta)) {
                *value = Some(next);
            }
        })
    };
    let grid_key = move |ev: leptos::ev::KeyboardEvent| {
        if ev.is_composing() || ev.ctrl_key() || ev.alt_key() || ev.meta_key() {
            return;
        }
        let key = ev.key();
        let by = match key.as_str() {
            "ArrowLeft" => -1,
            "ArrowRight" => 1,
            "ArrowUp" => -7,
            "ArrowDown" => 7,
            "PageUp" | "PageDown" => 0,
            _ => return,
        };
        ev.prevent_default();
        if by == 0 {
            step(if key == "PageUp" { -1 } else { 1 });
        } else {
            month.update(|m| {
                if let Some(m) = m {
                    m.move_days(by);
                }
            });
        }
        request_animation_frame(move || {
            if super::pickers::focused().is_none_or(|el| {
                el.tag_name() == "BODY" || el.closest(".qa-calendar-grid").ok().flatten().is_some()
            }) {
                platform::focus(ACTIVE_DAY);
            }
        });
    };
    view! {
        <div class="qa-presets">{move || today.get().map(|today| calendar::presets(today).into_iter().map(|preset| view! {
            <button type="button" class="qa-preset" class:current=move || due.get() == preset.date
                aria-current=move || (due.get() == preset.date).then_some("true")
                on:click=move |_| choose.run(preset.date)>
                {preset.label}<span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
            </button>
        }).collect_view())}</div>
        <div class="qa-month-title">
            <button type="button" class="qa-tool" aria-label="Previous month" on:click=move |_| step(-1)
                disabled=move || month.get().and_then(|m| m.shifted(-1)).is_none()>{icon(Icon::ChevronLeft)}</button>
            <span id="qa-month-label" aria-live="polite">{move || days.get().map(calendar::Month::label)}</span>
            <button type="button" class="qa-tool" aria-label="Next month" on:click=move |_| step(1)
                disabled=move || month.get().and_then(|m| m.shifted(1)).is_none()>{icon(Icon::ChevronRight)}</button>
        </div>
        <div class="qa-calendar-grid" role="grid" aria-labelledby="qa-month-label" on:keydown=grid_key>
            {move || days.get().map(|m| view! {
                <div class="qa-week" role="row">
                    {m.weekdays().into_iter().map(|label| view! { <span role="columnheader">{label}</span> }).collect_view()}
                </div>
                {m.rows().into_iter().map(|week| view! {
                    <div class="qa-week" role="row">
                        {week.into_iter().map(|cell| match cell {
                            Some(day) => view! {
                                <div role="gridcell" aria-selected=move || (due.get() == Some(day)).to_string()>
                                    <button type="button" class="qa-day" class:current=move || due.get() == Some(day)
                                        aria-label=calendar::day_label(day)
                                        aria-current=move || (today.get() == Some(day)).then_some("date")
                                        id=move || month.get().is_some_and(|m| m.cursor == day).then_some(ACTIVE_DAY)
                                        tabindex=move || if month.get().is_some_and(|m| m.cursor == day) { "0" } else { "-1" }
                                        on:click=move |_| choose.run(Some(day))>{day.day().to_string()}</button>
                                </div>
                            }.into_any(),
                            None => view! { <div role="gridcell"></div> }.into_any(),
                        }).collect_view()}
                    </div>
                }).collect_view()}
            })}
        </div>
    }
}
