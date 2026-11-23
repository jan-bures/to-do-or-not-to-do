//! The shared calendar for room composers and task detail, and the composer priority control.

use gpui::{
    ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Role, Window, div, prelude::*, px,
};
pub(super) use quick_add::Pick;
use todo_client::intent::{calendar, quick_add};
use todo_core::{ids::TaskId, model::Priority, time::Date};

use super::{ShellView, menu::MenuKind};
use crate::model::hints::Stop;
use crate::theme::{Icon, Palette, TextStyled, component, desktop, elevation, radius, space, ty};
use crate::ui::kit;

pub(super) struct Controls {
    pub input: FocusHandle,
    pub remove_date: FocusHandle,
    pub remove_priority: FocusHandle,
    pub date: FocusHandle,
    pub priority: FocusHandle,
}

impl Controls {
    pub(super) fn new(input: FocusHandle, cx: &Context<ShellView>) -> Self {
        Self {
            input,
            remove_date: cx.focus_handle(),
            remove_priority: cx.focus_handle(),
            date: cx.focus_handle(),
            priority: cx.focus_handle(),
        }
    }

    pub(super) fn button_focused(&self, window: &Window) -> bool {
        [
            &self.remove_date,
            &self.remove_priority,
            &self.date,
            &self.priority,
        ]
        .into_iter()
        .any(|h| h.is_focused(window))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Calendar {
    /// None for a composer draft; otherwise the task whose due date is being edited.
    pub(super) task: Option<TaskId>,
    pub(super) starts: bool,
    range: bool,
    today: Date,
    selected: Option<Date>,
    month: calendar::Month,
    focus: usize,
}

impl Calendar {
    pub(super) fn new(task: Option<TaskId>, today: Date, selected: Option<Date>) -> Self {
        Self {
            task,
            starts: false,
            range: false,
            today,
            selected,
            month: calendar::Month::new(selected.unwrap_or(today)),
            focus: 0,
        }
    }

    pub(super) fn for_start(mut self) -> Self {
        self.starts = true;
        self
    }

    fn activate(self) -> CalendarAction {
        let presets = calendar::presets(self.today);
        match self.focus {
            0 => CalendarAction::Pick(Some(self.month.cursor)),
            i if i <= presets.len() => CalendarAction::Pick(presets[i - 1].date),
            i if i == presets.len() + 1 => CalendarAction::Shift(-1),
            i if i == presets.len() + 2 => CalendarAction::Shift(1),
            i => CalendarAction::Endpoint(i == presets.len() + 3),
        }
    }

    fn navigate(&mut self, key: &str, reverse: bool) {
        match key {
            "tab" => {
                let stops =
                    calendar::presets(self.today).len() + 3 + if self.range { 2 } else { 0 };
                self.focus = (self.focus + if reverse { stops - 1 } else { 1 }) % stops;
            }
            "pageup" => self.shift(-1),
            "pagedown" => self.shift(1),
            "left" | "right" | "up" | "down" => {
                self.focus = 0;
                let by = match key {
                    "left" => -1,
                    "right" => 1,
                    "up" => -7,
                    _ => 7,
                };
                self.month.move_days(by);
            }
            _ => {}
        }
    }

    fn shift(&mut self, by: i32) {
        if let Some(next) = self.month.shifted(by) {
            self.month = next;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalendarAction {
    Pick(Option<Date>),
    Shift(i32),
    Endpoint(bool),
}

impl ShellView {
    pub(super) fn quick_preview(&self) -> quick_add::Parsed {
        self.quick_draft
            .parsed_with_features(&self.store.now().eval(), self.quick_features())
    }

    pub(super) fn quick_date_label(&self, draft: &quick_add::Parsed) -> Option<String> {
        let today = self.store.now().today();
        match (draft.start_date, draft.due) {
            (Some(start), Some(due)) => Some(crate::model::workflow::span(start, due, today)),
            (Some(start), None) => Some(format!("Starts {}", quick_add::short_date(start, today))),
            (None, due) => due.map(|d| quick_add::short_date(d, today)),
        }
    }

    pub(super) fn quick_calendar(&self) -> Calendar {
        let now = self.store.now().eval();
        let mut calendar = Calendar::new(None, now.today, self.quick_preview().due);
        calendar.range = self.quick_features().timeline;
        calendar
    }

    pub(super) fn quick_pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_picks([pick], window, cx);
    }

    fn quick_picks(
        &mut self,
        picks: impl IntoIterator<Item = Pick>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.store.now().eval();
        let features = self.quick_features();
        for pick in picks {
            self.quick_draft.pick(pick, &now, features);
        }
        let text = &self.quick_draft.text;
        if self.quick_add.read(cx).content.as_ref() != text.as_str() {
            self.quick_add
                .update(cx, |input, cx| input.set_text(text.clone(), cx));
        }
        self.close_menu(false, window, cx);
        window.focus(&self.quick_focus.input, cx);
        cx.notify();
    }

    pub(super) fn quick_picker_button(
        &self,
        date: bool,
        has_value: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let (id, stop, label, icon, focus) = if date {
            (
                "quick-due",
                Stop::QuickDue,
                "Choose due date",
                Icon::Calendar,
                &self.quick_focus.date,
            )
        } else {
            (
                "quick-priority",
                Stop::QuickPriority,
                "Choose priority",
                Icon::Flag,
                &self.quick_focus.priority,
            )
        };
        let belongs = |kind: &MenuKind| match kind {
            MenuKind::Date(c) => date && c.task.is_none(),
            MenuKind::QuickPriority => !date,
            _ => false,
        };
        let open = self
            .menu
            .as_ref()
            .is_some_and(|m| belongs(&m.kind) && m.overlay.look(self.frame).open);
        let button = div()
            .id(id)
            .track_focus(focus)
            .role(Role::Button)
            .aria_label(label)
            .aria_expanded(open)
            .tooltip(kit::tooltip(p, label))
            .cursor_pointer()
            .size(px(component::ICON_BUTTON))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if focus.is_focused(window) {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .when(open || has_value, |el| el.bg(p.wash))
            .hover(|el| el.bg(p.hover))
            .child(kit::icon(
                icon,
                component::ICON,
                if has_value { p.ink } else { p.ink_3 },
            ))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let kind = if date {
                    MenuKind::Date(this.quick_calendar())
                } else {
                    MenuKind::QuickPriority
                };
                this.toggle_menu(kind, stop, event, window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.control_key(event, window, cx)
            }));
        div()
            .relative()
            .flex_none()
            .child(button)
            .children(self.menu_anchor(belongs, cx))
    }

    pub(super) fn quick_date_chip(
        &self,
        label: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.quick_chip(
            label.clone(),
            format!("Remove due date: {label}"),
            true,
            window,
            cx,
        )
    }

    pub(super) fn quick_priority_chip(
        &self,
        level: Priority,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let option = quick_add::priority_option(level);
        self.quick_chip(
            option.mark.into(),
            format!("Remove priority: {}", option.label),
            false,
            window,
            cx,
        )
    }

    fn quick_chip(
        &self,
        label: String,
        name: String,
        date: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette;
        let (id, focus, ink, pick) = if date {
            (
                "quick-remove-date",
                &self.quick_focus.remove_date,
                p.accent_strong,
                Pick::Due(None),
            )
        } else {
            (
                "quick-remove-priority",
                &self.quick_focus.remove_priority,
                p.ink,
                Pick::Priority(Priority::None),
            )
        };
        div()
            .id(id)
            .track_focus(focus)
            .role(Role::Button)
            .aria_label(name.clone())
            .tooltip(kit::tooltip(p, name))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S2))
            .h(px(space::S6))
            .rounded(px(radius::SM))
            .bg(p.wash)
            .border(px(elevation::HAIRLINE))
            .border_color(if focus.is_focused(window) {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .text_color(ink)
            .style(ty::COUNT.with_weight(500))
            .cursor_pointer()
            .hover(|el| el.bg(p.hover))
            .when(date, |el| {
                el.child(kit::icon(Icon::Calendar, component::ICON_SMALL, ink))
            })
            .child(label)
            .child(kit::icon(Icon::X, component::ICON_SMALL, ink))
            .on_click(cx.listener(move |this, _, window, cx| {
                if date {
                    this.quick_picks([Pick::Start(None), pick], window, cx);
                } else {
                    this.quick_pick(pick, window, cx);
                }
            }))
    }

    pub(super) fn calendar_key(
        &mut self,
        mut calendar: Calendar,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e.keystroke.key.as_str() {
            "escape" => self.close_menu(true, window, cx),
            "enter" | "space" if !e.is_held => {
                self.calendar_action(calendar.activate(), window, cx)
            }
            key => {
                calendar.navigate(key, e.keystroke.modifiers.shift);
                if let Some(menu) = self.menu.as_mut() {
                    menu.kind = MenuKind::Date(calendar);
                }
                cx.notify();
            }
        }
    }

    fn calendar_action(
        &mut self,
        action: CalendarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self
            .menu
            .as_mut()
            .filter(|m| m.overlay.look(self.frame).open)
        else {
            return;
        };
        let MenuKind::Date(ref mut calendar) = menu.kind else {
            return;
        };
        match action {
            CalendarAction::Endpoint(starts) if calendar.range => {
                let parsed = self.quick_preview();
                let today = self.store.now().today();
                let mut next = Calendar::new(
                    None,
                    today,
                    if starts {
                        parsed.start_date
                    } else {
                        parsed.due
                    },
                );
                next.range = true;
                next.starts = starts;
                if let Some(menu) = &mut self.menu {
                    menu.kind = MenuKind::Date(next);
                }
                cx.notify();
            }
            CalendarAction::Endpoint(_) => {}
            CalendarAction::Pick(date) => {
                let starts = calendar.starts;
                if let Some(task) = calendar.task {
                    if starts {
                        self.choose_task_start(task, date, window, cx);
                    } else {
                        self.choose_task_due(task, date, window, cx);
                    }
                } else {
                    self.quick_pick(
                        if starts {
                            Pick::Start(date)
                        } else {
                            Pick::Due(date)
                        },
                        window,
                        cx,
                    );
                }
            }
            CalendarAction::Shift(by) => {
                calendar.shift(by);
                cx.notify();
            }
        }
    }

    fn calendar_repeat_note(&self, c: Calendar) -> Option<&'static str> {
        let task = c.task.filter(|_| !c.starts)?;
        self.store.view().tasks.get(&task)?.recurrence.as_ref()?;
        Some(calendar::REPEAT_CLEAR_NOTE)
    }

    pub(super) fn calendar_body(&self, c: Calendar, live: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let selected = c.selected;
        let repeat_note = self.calendar_repeat_note(c);
        let presets = calendar::presets(c.today);
        let preset_count = presets.len();
        let presets = presets
            .chunks(2)
            .enumerate()
            .map(|(row, pair)| {
                div()
                    .flex()
                    .children(pair.iter().enumerate().map(|(col, preset)| {
                        let i = row * 2 + col + 1;
                        self.calendar_control(
                            CalendarAction::Pick(preset.date),
                            ("date-preset", i),
                            if c.starts && preset.date.is_none() {
                                "Clear".to_owned()
                            } else {
                                preset.label.to_owned()
                            },
                            c.focus == i,
                            selected == preset.date,
                            live,
                            cx,
                        )
                        .relative()
                        .flex_1()
                        .when_some(
                            repeat_note.filter(|_| live && preset.date.is_none()),
                            |el, note| el.aria_description(note),
                        )
                        .style(ty::BUTTON_SMALL)
                        .child(preset.label)
                        .when(selected == preset.date, |el| {
                            el.child(div().absolute().right(px(space::S2)).child(kit::icon(
                                Icon::Check,
                                component::ICON_SMALL,
                                p.accent_strong,
                            )))
                        })
                    }))
            })
            .collect::<Vec<_>>();
        let month_button = |by, icon, name: &str, i, cx: &mut Context<Self>| {
            self.calendar_control(
                CalendarAction::Shift(by),
                ("calendar-month", i),
                name.to_owned(),
                c.focus == i,
                false,
                live && c.month.shifted(by).is_some(),
                cx,
            )
            .w(px(component::ICON_BUTTON))
            .child(kit::icon(icon, component::ICON, p.ink_3))
        };
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .py(px(space::S1))
            .child(month_button(
                -1,
                Icon::ChevronLeft,
                "Previous month",
                preset_count + 1,
                cx,
            ))
            .child(
                div()
                    .text_color(p.ink)
                    .style(ty::BUTTON)
                    .child(c.month.label()),
            )
            .child(month_button(
                1,
                Icon::ChevronRight,
                "Next month",
                preset_count + 2,
                cx,
            ));
        let weekdays = div()
            .flex()
            .children(c.month.weekdays().into_iter().map(|name| {
                div()
                    .flex_1()
                    .min_w_0()
                    .text_center()
                    .style(ty::COUNT)
                    .text_color(p.ink_3)
                    .child(name)
            }));
        let rows = c
            .month
            .rows()
            .into_iter()
            .enumerate()
            .map(|(row, days)| {
                div()
                    .flex()
                    .children(days.into_iter().enumerate().map(|(col, day)| {
                        let cell = div().flex_1().min_w_0();
                        if let Some(day) = day {
                            let label = calendar::day_label(day);
                            let focused = c.focus == 0 && day == c.month.cursor;
                            cell.child(
                                self.calendar_control(
                                    CalendarAction::Pick(Some(day)),
                                    ("calendar-day", row * 7 + col),
                                    label,
                                    focused,
                                    selected == Some(day),
                                    live,
                                    cx,
                                )
                                .w_full()
                                .style(ty::COUNT)
                                .when(!focused && day == c.today, |el| el.border_color(p.control))
                                .child(day.day().to_string()),
                            )
                        } else {
                            cell
                        }
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .px(px(space::S2))
            .pb(px(space::S2))
            .when(c.range, |d| {
                d.child(
                    div().flex().gap(px(space::S1)).pb(px(space::S1)).children(
                        [(true, "Starts"), (false, "Due")]
                            .into_iter()
                            .enumerate()
                            .map(|(i, (starts, label))| {
                                self.calendar_control(
                                    CalendarAction::Endpoint(starts),
                                    ("calendar-endpoint", i),
                                    label.into(),
                                    c.focus == preset_count + 3 + i,
                                    c.starts == starts,
                                    live,
                                    cx,
                                )
                                .flex_1()
                                .style(ty::BUTTON_SMALL)
                                .child(label)
                            }),
                    ),
                )
            })
            .children(presets)
            .child(
                div()
                    .h(px(elevation::HAIRLINE))
                    .bg(p.line)
                    .mt(px(space::S1)),
            )
            .child(header)
            .child(weekdays)
            .children(rows)
            .when_some(repeat_note, |el, note| {
                el.child(
                    div()
                        .mt(px(space::S1))
                        .pt(px(space::S2))
                        .border_t(px(elevation::HAIRLINE))
                        .border_color(p.line)
                        .style(ty::META)
                        .text_color(p.ink_3)
                        .child(note),
                )
            })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one shared control for presets, navigation and days"
    )]
    fn calendar_control(
        &self,
        action: CalendarAction,
        id: (&'static str, usize),
        label: String,
        focused: bool,
        selected: bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p: Palette = self.palette;
        div()
            .id(id)
            .h(px(desktop::NAV_ROW))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if focused {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .text_color(if selected { p.accent_strong } else { p.ink })
            .when(selected, |el| {
                el.bg(p.wash).font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .when(live, |el| {
                el.role(Role::Button)
                    .aria_label(label)
                    .aria_selected(selected)
                    .when(focused, |el| el.aria_active_descendant())
                    .cursor_pointer()
                    .hover(|el| el.bg(p.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.calendar_action(action, window, cx)
                    }))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(cx: &mut gpui::VisualTestContext, key: &'static str) {
        cx.simulate_keystrokes(key);
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).expect("key"),
        });
    }

    #[gpui::test]
    fn picker_keys_change_only_the_draft_until_quick_add_submits(cx: &mut gpui::TestAppContext) {
        cx.update(|app| app.bind_keys(crate::ui::input::key_bindings()));
        let fixture = crate::fixtures::by_name("quick-add-chips").expect("composer fixture");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = std::rc::Rc::new(crate::store::Store::fixture(&fixture));
            let mut view = ShellView::new(
                store,
                fixture.ui,
                crate::theme::Theme::Light,
                Some("Water the balcony plants zítra !!"),
                window,
                cx,
            );
            view.still();
            view
        });
        let task_count = cx.update(|window, cx| {
            let view = shell.read(cx);
            let count = view.store.view().tasks.len();
            window.focus(&view.quick_focus.date.clone(), cx);
            count
        });
        press(cx, "enter");
        press(cx, "pagedown");
        press(cx, "escape");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert!(view.quick_focus.date.is_focused(window));
            assert_eq!(view.quick_draft.text, "Water the balcony plants zítra !!");
        });
        press(cx, "enter");
        press(cx, "right");
        press(cx, "enter");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert!(view.quick_focus.input.is_focused(window));
            assert_eq!(view.quick_draft.text, "Water the balcony plants !!");
            assert_eq!(view.quick_preview().due, Some(Date::constant(2026, 11, 23)));
            assert_eq!(view.store.view().tasks.len(), task_count);
            window.focus(&view.quick_focus.priority.clone(), cx);
        });
        press(cx, "enter");
        press(cx, "up");
        press(cx, "enter");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.quick_draft.text, "Water the balcony plants");
            assert_eq!(view.quick_preview().priority, Some(Priority::High));
            assert_eq!(view.store.view().tasks.len(), task_count);
        });
        press(cx, "enter");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            let state = view.store.view();
            assert_eq!(state.tasks.len(), task_count + 1);
            let task = state
                .tasks
                .values()
                .find(|task| task.title == "Water the balcony plants")
                .expect("task");
            assert_eq!(task.due, Some(Date::constant(2026, 11, 23)));
            assert_eq!(task.priority, Priority::High);
            assert_eq!(view.quick_draft, quick_add::Draft::default());
            assert!(view.quick_focus.input.is_focused(window));
        });
    }

    #[gpui::test]
    fn detail_calendar_keeps_the_draft_separate_and_undo_restores_the_schedule(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| app.bind_keys(super::super::key_bindings()));
        let fixture = crate::fixtures::by_name("detail-due-calendar").expect("detail fixture");
        let task = fixture.ui.task.expect("selected task");
        let due = fixture.state.tasks[&task].due.expect("initial due");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                std::rc::Rc::new(crate::store::Store::fixture(&fixture)),
                fixture.ui,
                crate::theme::Theme::Light,
                None,
                window,
                cx,
            );
            view.still();
            view
        });
        cx.update(|window, cx| window.focus(&shell.read(cx).list_focus.clone(), cx));
        press(cx, "d");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert!(view.menu_focus.is_focused(window));
            let MenuKind::Date(calendar) = view.menu.as_ref().expect("date picker").kind else {
                panic!("expected the calendar");
            };
            assert_eq!(calendar.task, Some(task));
            assert_eq!(calendar.month.cursor, due);
            assert_eq!(calendar.selected, Some(due));
            assert_eq!(
                view.calendar_repeat_note(calendar),
                Some(calendar::REPEAT_CLEAR_NOTE)
            );
            assert_eq!(view.calendar_repeat_note(view.quick_calendar()), None);
        });
        press(cx, "pagedown");
        press(cx, "escape");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert!(view.due_focus.is_focused(window));
            assert_eq!(view.store.view().tasks[&task].due, Some(due));
        });
        press(cx, "enter");
        press(cx, "right");
        press(cx, "enter");
        let changed = todo_core::time::add_days(due, 1);
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.store.view().tasks[&task].due, changed);
            assert_eq!(view.quick_draft, quick_add::Draft::default());
        });
        press(cx, "enter");
        for _ in 0..4 {
            press(cx, "tab");
        }
        press(cx, "enter");
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert_eq!(state.tasks[&task].due, None);
            assert_eq!(state.tasks[&task].recurrence, None);
        });
        press(cx, "enter");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            let MenuKind::Date(calendar) = view.menu.as_ref().expect("date picker").kind else {
                panic!("expected the calendar");
            };
            assert_eq!(view.calendar_repeat_note(calendar), None);
        });
        press(cx, "escape");
        press(cx, "ctrl-z");
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert_eq!(state.tasks[&task].due, changed);
            assert!(state.tasks[&task].recurrence.is_some());
        });
    }

    #[test]
    fn calendar_arrows_page_keys_and_preset_activation_are_separate() {
        let mut c = Calendar::new(
            None,
            Date::constant(2026, 11, 21),
            Some(Date::constant(2026, 11, 30)),
        );
        c.navigate("right", false);
        assert_eq!(
            c.activate(),
            CalendarAction::Pick(Some(Date::constant(2026, 12, 1)))
        );
        c.navigate("pageup", false);
        c.navigate("down", false);
        assert_eq!(
            c.activate(),
            CalendarAction::Pick(Some(Date::constant(2026, 11, 8)))
        );
        for _ in 0..4 {
            c.navigate("tab", false);
        }
        assert_eq!(c.activate(), CalendarAction::Pick(None));
    }
}
