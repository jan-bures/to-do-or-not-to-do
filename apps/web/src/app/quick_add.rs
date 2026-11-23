//! The room composer, its priority picker and the date picker shared with task detail.

use leptos::prelude::*;
use todo_client::intent::{self, calendar, quick_add};
use todo_core::{model::Priority, query::QueryStatus, time::Date};
use todo_design::Icon;

use super::pickers::{DetailPicker, choice_focus};
use super::{Ctx, backend, ctx, icon, platform, workflow};

const INPUT: &str = "qa-input";

#[derive(Clone, Copy)]
pub(super) struct DatePicker {
    pub(super) popup: DetailPicker,
    month: RwSignal<Option<calendar::Month>>,
    today: RwSignal<Option<Date>>,
    due: RwSignal<Option<Date>>,
    active_day: &'static str,
    month_label: &'static str,
}

impl DatePicker {
    pub(super) fn new(
        popup: DetailPicker,
        active_day: &'static str,
        month_label: &'static str,
    ) -> Self {
        Self {
            popup,
            month: RwSignal::new(None),
            today: RwSignal::new(None),
            due: RwSignal::new(None),
            active_day,
            month_label,
        }
    }

    pub(super) fn toggle(self, today: Date, due: Option<Date>) {
        if self.popup.open.get_untracked() {
            self.popup.close(true);
        } else {
            self.show(today, due);
        }
    }

    pub(super) fn show(self, today: Date, due: Option<Date>) {
        self.today.set(Some(today));
        self.show_date(today, due);
        self.popup.show(self.active_day);
    }

    fn show_date(self, today: Date, day: Option<Date>) {
        self.due.set(day);
        self.month
            .set(Some(calendar::Month::new(day.unwrap_or(today))));
    }
}

#[component]
pub(super) fn QuickAdd(#[prop(optional)] column: Option<QueryStatus>) -> impl IntoView {
    let c = ctx();
    let initial = move || quick_add::Draft {
        status: column.and_then(workflow::open_stage),
        ..Default::default()
    };
    let draft = RwSignal::new(initial());
    let calendar = DatePicker::new(
        DetailPicker::anchored("qa-date", "qa-calendar"),
        "qa-active-day",
        "qa-month-label",
    );
    let date_picker = calendar.popup;
    let priority_picker = DetailPicker::anchored("qa-priority", "qa-priority-menu");
    let status_picker = DetailPicker::anchored("qa-status", "qa-status-menu");
    let add_placeholder = Memo::new(move |_| {
        column
            .map(|s| format!("Add to {}", workflow::stage_info(s).0))
            .or_else(|| c.view_room.with(|r| r.add_placeholder.clone()))
    });
    let room = Memo::new(move |_| c.room.get());
    let features = Memo::new(move |_| {
        c.state
            .with(|state| intent::quick_add_features(state, (&room.get()).into()))
    });
    let starts = RwSignal::new(false);
    Effect::new(move |_| {
        room.track();
        features.track();
        if c.read_only() || date_picker.open.get_untracked() {
            date_picker.close(false);
        }
        if c.read_only() || priority_picker.open.get_untracked() {
            priority_picker.close(false);
        }
        if c.read_only() || status_picker.open.get_untracked() {
            status_picker.close(false);
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
                intent::submit_workflow_draft(
                    state,
                    &room.get_untracked(),
                    draft,
                    &eval,
                    backend::new_uuid,
                )
            })
        });
        if id.is_some() && commands.into_iter().all(|command| c.submit(command)) {
            draft.set(initial());
        }
    };
    let parsed = Memo::new(move |_| {
        draft.with(|draft| draft.parsed_with_features(&c.now.get().eval(), features.get()))
    });
    let due = Memo::new(move |_| parsed.with(|p| p.due));
    let start = Memo::new(move |_| parsed.with(|p| p.start_date));
    let priority = Memo::new(move |_| parsed.with(|p| p.priority.unwrap_or_default()));
    let status = Memo::new(move |_| {
        parsed.with(|p| {
            p.status
                .map(workflow::stage)
                .or(column)
                .unwrap_or(QueryStatus::Todo)
        })
    });
    let status_chip =
        move || features.get().workflow && status.get() != column.unwrap_or(QueryStatus::Todo);
    let date =
        Memo::new(move |_| quick_add::date_preview(start.get(), due.get(), c.now.get().dates()));
    let chip_label = Memo::new(move |prev: Option<&String>| {
        date.get().or_else(|| prev.cloned()).unwrap_or_default()
    });
    let pick = move |choice| {
        if !c.read_only() {
            draft.update(|d| d.pick(choice, &now().eval(), features.get_untracked()));
        }
    };
    let choose_day = move |day| {
        pick(if starts.get_untracked() {
            quick_add::Pick::Start(day)
        } else {
            quick_add::Pick::Due(day)
        });
        date_picker.close(false);
        platform::focus(INPUT);
    };
    let clear_dates = move || {
        pick(quick_add::Pick::Start(None));
        pick(quick_add::Pick::Due(None));
        platform::focus(INPUT);
    };
    let choose_priority = move |level| {
        pick(quick_add::Pick::Priority(level));
        priority_picker.close(false);
        platform::focus(INPUT);
    };
    let choose_status = move |s| {
        pick(quick_add::Pick::Status(workflow::open_stage(s)));
        status_picker.close(false);
        platform::focus(INPUT);
    };
    let show_date = move || {
        if !c.read_only() {
            starts.set(false);
            calendar.toggle(now().today(), due.get_untracked());
        }
    };
    let switch_date = Callback::new(move |is_start| {
        starts.set(is_start);
        calendar.show_date(
            now().today(),
            if is_start {
                start.get_untracked()
            } else {
                due.get_untracked()
            },
        );
    });
    let show_priority = move || {
        if priority_picker.open.get_untracked() {
            priority_picker.close(true);
        } else if !c.read_only() {
            priority_picker.show_choice(priority_picker.id);
        }
    };
    view! {
        <Show when=move || add_placeholder.with(Option::is_some)>
            <form class="quick-add" class:column-quick-add=column.is_some()
                on:submit=move |ev| { ev.prevent_default(); add(); }
                on:keydown=move |ev| {
                    if let Some(stage) = column && ev.key() == "Escape" && !ev.is_composing() {
                        ev.prevent_default(); ev.stop_propagation();
                        c.board_ui.composer.set(None);
                        request_animation_frame(move || { platform::focus(super::board::add_id(stage)); });
                    }
                }>
                <span class="qa-plus">{icon(Icon::Plus)}</span>
                <input node_ref=c.quick_add id=INPUT class="qa-input" type="text"
                    disabled=move || c.read_only() autocomplete="off" enterkeyhint="done"
                    aria-label=move || column.map_or_else(|| "Add a task".into(), |_| add_placeholder.get().unwrap_or_default())
                    placeholder=move || add_placeholder.get().unwrap_or_default()
                    prop:value=move || draft.with(|d| d.text.clone())
                    on:input=move |ev| draft.update(|d| d.edit_with_features(event_target_value(&ev), &c.now.get_untracked().eval(), features.get_untracked())) />
                <span class="qa-date-announcement" role="status">{move || {
                    let mut labels = Vec::new();
                    if let Some(date) = date.get() { labels.push(if due.get().is_some() { format!("Due {date}") } else { date }); }
                    if status_chip() { labels.push(workflow::stage_info(status.get()).0.into()); }
                    if priority.get() != Priority::None {
                        labels.push(format!("{} priority", quick_add::priority_option(priority.get()).label));
                    }
                    labels.join(". ")
                }}</span>
                <div class="qa-chips" class:has-chips=move || date.with(Option::is_some) || status_chip() || priority.get() != Priority::None>
                    <Show when=status_chip>
                        <button type="button" class="qa-status-chip" disabled=move || c.read_only()
                            aria-label=move || format!("Remove status: {}", workflow::stage_info(status.get()).0)
                            on:click=move |_| choose_status(column.unwrap_or(QueryStatus::Todo))>
                            {move || workflow::mark(status.get())}{icon(Icon::X)}
                        </button>
                    </Show>
                    <button type="button" class="qa-date-chip" class:no-date=move || date.with(Option::is_none)
                        disabled=move || date.with(Option::is_none) || c.read_only()
                        aria-label=move || format!("Remove {}: {}", if start.get().is_some() { "dates" } else { "due date" }, chip_label.get())
                        on:click=move |_| clear_dates()>
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
                    <Show when=move || features.get().workflow>
                        <button type="button" id=status_picker.anchor class="qa-tool" class:has-value=status_chip
                            title="Choose status" aria-label="Choose status" aria-haspopup="listbox" aria-controls=status_picker.id
                            aria-expanded=move || status_picker.open.get().to_string() disabled=move || c.read_only()
                            on:click=move |_| {
                                if status_picker.open.get_untracked() { status_picker.close(true); }
                                else if !c.read_only() { status_picker.show_choice(status_picker.id); }
                            }>{move || icon(workflow::stage_info(status.get()).2)}</button>
                    </Show>
                    <button type="button" id=date_picker.anchor data-calendar-anchor="" class="qa-tool" class:has-value=move || date.with(Option::is_some)
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
                <CalendarPopover c=c picker=calendar choose=Callback::new(choose_day)
                    starts=Signal::derive(move || starts.get()) range=Signal::derive(move || features.get().timeline) switch_date=switch_date/>
                <div class="room-action-menu status-menu" popover="auto" id=status_picker.id node_ref=status_picker.node
                    role="listbox" aria-label="Choose status" on:toggle=move |_| status_picker.toggle_event()
                    on:focusin=choice_focus on:keydown=move |ev| status_picker.key(c, Some(1), &ev)>
                    <workflow::StatusOptions current=Signal::derive(move || Some(status.get())) choose=Callback::new(choose_status) open_only=true/>
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
pub(super) fn CalendarPopover(
    c: Ctx,
    picker: DatePicker,
    choose: Callback<Option<Date>>,
    #[prop(optional)] repeats: Signal<bool>,
    #[prop(optional)] starts: Signal<bool>,
    #[prop(optional)] range: Signal<bool>,
    #[prop(optional)] switch_date: Option<Callback<bool>>,
) -> impl IntoView {
    let DatePicker {
        popup,
        month,
        today,
        due,
        active_day,
        month_label,
    } = picker;
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
            if popup.open.try_get_untracked() == Some(true)
                && super::pickers::focused().is_none_or(|el| {
                    el.tag_name() == "BODY"
                        || el
                            .closest(".qa-calendar-grid")
                            .ok()
                            .flatten()
                            .is_some_and(|grid| {
                                grid.get_attribute("aria-labelledby").as_deref()
                                    == Some(month_label)
                            })
                })
            {
                platform::focus(active_day);
            }
        });
    };
    view! {
        // Keep shell shortcuts out from the instant the popover opens, before focusing the day on the next frame.
        <div class="room-action-menu qa-calendar" popover="auto" id=popup.id node_ref=popup.node
            role="dialog" aria-label=move || if starts.get() { "Choose start date" } else { "Choose due date" }
            tabindex="-1" autofocus on:toggle=move |_| popup.toggle_event()
            on:keydown=move |ev| popup.key(c, None, &ev)>
        <Show when=move || range.get()>
            <div class="view-switcher calendar-range" role="group" aria-label="Date field">
                {[true, false].map(|value| view! {
                    <button type="button" aria-pressed=move || (starts.get() == value).to_string()
                        on:click=move |_| { if let Some(switch) = switch_date { switch.run(value); } }>
                        {if value { "Starts" } else { "Due" }}
                    </button>
                })}
            </div>
        </Show>
        <div class="qa-presets">{move || today.get().map(|today| calendar::presets(today).into_iter().map(|preset| view! {
            <button type="button" class="qa-preset" class:current=move || due.get() == preset.date
                aria-current=move || (due.get() == preset.date).then_some("true")
                aria-describedby=move || (preset.date.is_none() && repeats.get()).then_some("calendar-repeat-note")
                on:click=move |_| choose.run(preset.date)>
                {move || if preset.date.is_none() && starts.get() { "Clear" } else { preset.label }}
                <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
            </button>
        }).collect_view())}</div>
        <div class="qa-month-title">
            <button type="button" class="qa-tool" aria-label="Previous month" on:click=move |_| step(-1)
                disabled=move || month.get().and_then(|m| m.shifted(-1)).is_none()>{icon(Icon::ChevronLeft)}</button>
            <span id=month_label aria-live="polite">{move || days.get().map(|m| c.now.get().dates().month(m.cursor))}</span>
            <button type="button" class="qa-tool" aria-label="Next month" on:click=move |_| step(1)
                disabled=move || month.get().and_then(|m| m.shifted(1)).is_none()>{icon(Icon::ChevronRight)}</button>
        </div>
        <div class="qa-calendar-grid" role="grid" aria-labelledby=month_label on:keydown=grid_key>
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
                                        id=move || month.get().is_some_and(|m| m.cursor == day).then_some(active_day)
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
        <Show when=move || repeats.get()>
            <p class="calendar-repeat-note" id="calendar-repeat-note">{calendar::REPEAT_CLEAR_NOTE}</p>
        </Show>
        </div>
    }
}
