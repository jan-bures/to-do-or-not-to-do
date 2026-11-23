//! List settings, view switching and the workflow controls shared by List and Board.

use gpui::{
    AnyElement, ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Role, Stateful, Window, div,
    prelude::*, px,
};
use todo_client::intent::{self, workflow as intent_workflow};
use todo_core::command::Placement;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{ListFeatures, ListView};
use todo_core::query::QueryStatus;
use todo_core::time::Date;

use super::{
    MenuKind, RoomKey, ShellView, component, desktop, elevation, hints::Stop, kit, radius, space,
    ty,
};
use crate::model::{
    FieldEdit,
    menus::{MenuAction, MenuItem},
    workflow::{self, Key, Setting},
};
use crate::theme::{Icon, Palette, TextStyled};

pub(super) use crate::model::workflow::open_stage;

pub(super) struct Controls {
    pub view: [FocusHandle; 3],
    pub quick: FocusHandle,
    pub clear: FocusHandle,
    pub status: FocusHandle,
    pub starts: FocusHandle,
}

impl Controls {
    pub fn new(cx: &Context<ShellView>) -> Self {
        Self {
            view: std::array::from_fn(|_| cx.focus_handle()),
            quick: cx.focus_handle(),
            clear: cx.focus_handle(),
            status: cx.focus_handle(),
            starts: cx.focus_handle(),
        }
    }
}

pub(super) fn status_ink(p: &Palette, stage: QueryStatus, selected: bool) -> gpui::Hsla {
    match stage {
        QueryStatus::Backlog => p.ink_3,
        QueryStatus::Todo => p.ink_2,
        QueryStatus::InProgress => p.accent_strong,
        QueryStatus::Done if selected => p.check_selected,
        QueryStatus::Done => p.accent,
    }
}

pub(super) fn status_mark(p: &Palette, stage: QueryStatus, selected: bool) -> Div {
    let (label, icon) = workflow::status(stage);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::S1))
        .style(ty::META.with_weight(500))
        .text_color(if stage == QueryStatus::Done {
            p.ink_2
        } else {
            status_ink(p, stage, selected)
        })
        .when(stage == QueryStatus::InProgress, |d| {
            d.rounded(px(radius::SM))
                .bg(if selected { p.raised } else { p.wash })
                .h(px(component::CHIP))
                .px(px(component::CHIP_GAP))
        })
        .child(kit::icon(
            icon,
            component::ICON_SMALL,
            status_ink(p, stage, selected),
        ))
        .child(label)
}

pub(super) fn mark_width(window: &Window, stage: QueryStatus) -> f32 {
    let (label, _) = workflow::status(stage);
    component::ICON_SMALL
        + space::S1
        + kit::text_width(window, ty::META.with_weight(500), label)
        + if stage == QueryStatus::InProgress {
            2. * component::CHIP_GAP
        } else {
            0.
        }
}

impl ShellView {
    pub(super) fn quick_features(&self) -> ListFeatures {
        intent::quick_add_features(&self.store.view(), self.ui.room.into())
    }

    pub(super) fn quick_stage(&self) -> QueryStatus {
        workflow::stage(self.quick_preview().status.unwrap_or_default())
    }

    pub(super) fn task_stage(&self, task: TaskId) -> Option<QueryStatus> {
        let state = self.store.view();
        let task = state.tasks.get(&task)?;
        (intent::organize::is_actionable(&state, task)
            && state.lists.get(&task.list)?.features.workflow)
            .then(|| QueryStatus::of(task))
    }

    pub(super) fn task_has_start(&self, task: TaskId) -> bool {
        let state = self.store.view();
        state.tasks.get(&task).is_some_and(|task| {
            intent::organize::is_actionable(&state, task)
                && state
                    .lists
                    .get(&task.list)
                    .is_some_and(|l| l.features.timeline)
        })
    }

    pub(super) fn view(&self) -> ListView {
        if self.ui.compact {
            return ListView::List;
        }
        workflow::effective(self.shell.room.features, self.board.view_override)
    }

    pub(super) fn on_board(&self) -> bool {
        self.view() == ListView::Board
    }

    pub(super) fn switch_view(
        &mut self,
        view: ListView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view() == view
            || !workflow::available(self.shell.room.features, view)
            || self.ui.compact
        {
            return;
        }
        if !self.commit_edits(cx) {
            return self.keep_unsaved(cx);
        }
        self.close_menu(false, window, cx);
        self.cancel_task_drag(cx);
        self.board.reset_view();
        self.timeline.reset_view();
        self.timeline.cursor = self.ui.task;
        self.board.view_override = Some(view);
        self.board.start_swap(self.now(), self.pace(), self.animate);
        self.multi.clear();
        self.rows_bar
            .reset_to_top(&self.rows.0.borrow().base_handle);
        // Preserve open detail, but seed the Board cursor from the selected task when possible.
        if let Some(id) = self.ui.task {
            self.aim_board_cursor(id);
        }
        self.refresh(cx);
        self.record_location();
        window.focus(&self.list_focus, cx);
    }

    pub(super) fn apply_list_setting(
        &mut self,
        list: ListId,
        setting: Setting,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        let view = self.view();
        if let Some(l) = state.lists.get(&list)
            && let Some(cmd) =
                intent_workflow::set_list_features(&state, list, setting.patch(l.features))
            && self.submit(vec![cmd])
            && self.ui.room == RoomKey::List(list)
        {
            let next = setting.patch(l.features).merge(l.features);
            // Saving a default changes the next visit, not the room open behind the menu.
            if matches!(setting, Setting::Default(_)) {
                self.board.view_override = Some(view);
            } else if self
                .board
                .view_override
                .is_some_and(|v| !workflow::available(next, v))
            {
                self.board.view_override = None;
            }
        }
        self.refresh(cx);
    }

    pub(super) fn view_switcher(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let features = self.shell.room.features;
        if !features.workflow && !features.timeline {
            return None;
        }
        let p = self.palette;
        let compact = self.room_width(window) < desktop::LIST_MIN;
        Some(
            div().child(
                div()
                    .id("view-switcher")
                    .role(Role::TabList)
                    .aria_label("Room view")
                    .h(px(component::VIEW_SWITCHER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(elevation::HAIRLINE))
                    .p(px(elevation::HAIRLINE + elevation::HAIRLINE))
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line)
                    .bg(p.canvas)
                    .children(
                        workflow::VIEWS
                            .into_iter()
                            .enumerate()
                            .filter(|(_, view)| features.allows(*view))
                            .map(|(i, view)| {
                                let (label, icon) = workflow::view_label(view);
                                let active = self.view() == view;
                                let focus = &self.workflow.view[i];
                                div()
                                    .id(("room-view", i))
                                    .role(Role::Tab)
                                    .aria_label(label)
                                    .aria_selected(active)
                                    .tooltip(kit::tooltip(p, label))
                                    .flex_none()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(space::S1))
                                    .rounded(px(radius::SM))
                                    .border(px(elevation::HAIRLINE))
                                    .border_color(if focus.is_focused(window) {
                                        p.accent
                                    } else if active {
                                        p.line_strong
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .when(active, |d| d.bg(p.raised))
                                    .when(compact, |d| d.w(px(component::VIEW_SWITCHER)))
                                    .when(!compact, |d| d.px(px(space::S2)))
                                    .style(ty::BUTTON_SMALL.with_weight(if active {
                                        600
                                    } else {
                                        500
                                    }))
                                    .text_color(if active { p.ink } else { p.ink_2 })
                                    .child(kit::icon(
                                        icon,
                                        component::ICON_SMALL,
                                        if active { p.accent_strong } else { p.ink_3 },
                                    ))
                                    .when(!compact, |d| d.child(label))
                                    .track_focus(focus)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(p.hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.switch_view(view, window, cx)
                                    }))
                                    .on_key_down(cx.listener(
                                        move |this, e: &KeyDownEvent, window, cx| {
                                            if !e.keystroke.modifiers.modified()
                                                && matches!(
                                                    e.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                )
                                            {
                                                cx.stop_propagation();
                                                if !e.is_held {
                                                    this.switch_view(view, window, cx);
                                                }
                                            }
                                        },
                                    ))
                            }),
                    ),
            ),
        )
    }

    pub(super) fn workflow_key(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let board = self.on_board();
        let Some((key, act)) = workflow::key(e, board) else {
            return false;
        };
        if !self.list_focus.is_focused(window) {
            return false;
        }
        let task = if board {
            self.board_task()
        } else {
            self.ui.task.filter(|&id| self.task_is_visible(id))
        };
        match key {
            Key::CycleView | Key::View(_)
                if self.ui.compact
                    || !(self.shell.room.features.workflow
                        || self.shell.room.features.timeline) =>
            {
                return false;
            }
            Key::Status | Key::CycleStatus if task.and_then(|id| self.task_stage(id)).is_none() => {
                return false;
            }
            Key::Starts if task.is_none_or(|id| self.ui.compact || !self.task_has_start(id)) => {
                return false;
            }
            _ => {}
        }
        if !act {
            return true;
        }
        match key {
            Key::CycleView => {
                let features = self.shell.room.features;
                let views = workflow::VIEWS
                    .into_iter()
                    .filter(|&v| workflow::available(features, v))
                    .collect::<Vec<_>>();
                if let Some(i) = views.iter().position(|&v| v == self.view()) {
                    self.switch_view(views[(i + 1) % views.len()], window, cx);
                }
            }
            Key::View(view) => self.switch_view(view, window, cx),
            Key::Status => {
                if let Some(task) = task {
                    let at = if board {
                        self.board_anchor(task, window)
                    } else {
                        self.row_anchor(task, window)
                    };
                    self.open_menu(
                        MenuKind::Status {
                            task: Some(task),
                            at_row: true,
                        },
                        Some(at),
                        Some(Stop::List),
                        window,
                        cx,
                    );
                    self.highlight_first();
                }
            }
            Key::CycleStatus => {
                if let Some((task, stage)) =
                    task.and_then(|id| self.task_stage(id).map(|stage| (id, stage)))
                {
                    self.choose_status(
                        task,
                        workflow::STAGES[(stage as usize + 1) % workflow::STAGES.len()],
                        cx,
                    );
                }
            }
            Key::Starts => {
                if let Some(task) = task {
                    if board {
                        self.select_task(Some(task), cx);
                    }
                    if self.ui.task == Some(task) {
                        self.scroll_into_view(&self.detail_scroll, "starts-row");
                        self.open_menu(
                            self.start_calendar(task),
                            None,
                            Some(Stop::Starts),
                            window,
                            cx,
                        );
                    }
                }
            }
            key => self.board_key(key, window, cx),
        }
        true
    }

    pub(super) fn row_status_mark(
        &self,
        stage: QueryStatus,
        selected: bool,
        row: Option<(RoomKey, TaskId)>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mark = status_mark(palette, stage, selected);
        let Some((room, task)) = row else {
            return mark.into_any_element();
        };
        let p = *palette;
        let (name, _) = workflow::status(stage);
        let label = format!("Status: {name}");
        let kind = MenuKind::Status {
            task: Some(task),
            at_row: true,
        };
        div()
            .id(gpui::SharedString::from(format!("row-status-{task}")))
            .flex_none()
            .cursor_pointer()
            .role(Role::Button)
            .aria_label(label.clone())
            .tooltip(kit::tooltip(p, label))
            .child(mark)
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                cx.stop_propagation();
                if this.row_can_act(room, task)
                    && !this.modified_task_click(task, e.modifiers(), cx)
                {
                    if this.menu_press.swallows(kind) {
                        return;
                    }
                    let at = e.mouse_position().map_or_else(
                        || this.row_anchor(task, window),
                        |pos| gpui::point(pos.x, pos.y + px(component::CHIP / 2.)),
                    );
                    this.open_menu(kind, Some(at), Some(Stop::List), window, cx);
                    this.highlight_first();
                }
            }))
            .into_any_element()
    }

    pub(super) fn choose_status(
        &mut self,
        task: TaskId,
        stage: QueryStatus,
        cx: &mut Context<Self>,
    ) {
        let Some(old) = self.task_stage(task).filter(|&old| old != stage) else {
            return;
        };
        if !self.commit_edits(cx) {
            return self.keep_unsaved(cx);
        }
        let state = self.store.view();
        let Some(t) = state.tasks.get(&task) else {
            return;
        };
        let list = t.list;
        if stage == QueryStatus::Done
            || (old == QueryStatus::Done && workflow::open_stage(stage) == Some(t.status))
        {
            self.toggle_complete(task, cx);
        } else if old == QueryStatus::Done {
            self.move_board_card(list, task, stage, Placement::Last, cx);
        } else if let Some(cmd) = workflow::open_stage(stage)
            .and_then(|status| intent_workflow::set_status(&state, task, status))
        {
            self.submit(vec![cmd]);
            self.refresh(cx);
        }
        if self.on_board() {
            self.aim_board_cursor(task);
        }
    }

    pub(super) fn start_calendar(&self, task: TaskId) -> MenuKind {
        let start = self
            .store
            .view()
            .tasks
            .get(&task)
            .and_then(|t| t.start_date);
        MenuKind::Date(
            super::quick_pickers::Calendar::new(Some(task), self.store.now().today(), start)
                .for_start(),
        )
    }

    pub(super) fn choose_task_start(
        &mut self,
        task: TaskId,
        start: Option<Date>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repeat_target() != Some(task) || !self.task_has_start(task) {
            self.close_menu(false, window, cx);
            return;
        }
        let cmd = intent_workflow::set_start(&self.store.view(), task, start);
        if cmd.is_none_or(|cmd| self.submit(vec![cmd])) {
            self.close_menu(true, window, cx);
        }
        self.refresh(cx);
    }

    pub(super) fn workflow_property(
        &self,
        row: Stateful<Div>,
        task: TaskId,
        edit: FieldEdit,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let starts = edit == FieldEdit::Starts;
        let (stop, focus, label, key, probe) = if starts {
            (
                Stop::Starts,
                &self.workflow.starts,
                "Edit start date",
                "⇧D",
                "starts-row",
            )
        } else {
            (
                Stop::Status,
                &self.workflow.status,
                "Choose status",
                "S",
                "status-row",
            )
        };
        let belongs = move |kind: &MenuKind| match kind {
            MenuKind::Date(c) if starts => c.timeline.is_none() && c.starts && c.task == Some(task),
            MenuKind::Status {
                task: Some(id),
                at_row: false,
            } if !starts => *id == task,
            _ => false,
        };
        let open = self
            .menu
            .as_ref()
            .is_some_and(|m| belongs(&m.kind) && m.overlay.look(self.frame).open);
        row.track_focus(focus)
            .role(Role::Button)
            .aria_label(label)
            .aria_expanded(open)
            .hover(|s| s.bg(p.hover))
            .cursor_pointer()
            .child(div().flex_1())
            .child(kit::kbd(&p, key))
            .child(self.probe(probe))
            .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)))
            .on_click(cx.listener(move |this, e, window, cx| {
                let kind = if starts {
                    this.start_calendar(task)
                } else {
                    MenuKind::Status {
                        task: Some(task),
                        at_row: false,
                    }
                };
                this.toggle_menu(kind, stop, e, window, cx);
            }))
            .children(self.menu_anchor(belongs, cx))
    }

    pub(super) fn quick_status_button(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let kind = MenuKind::Status {
            task: None,
            at_row: false,
        };
        let stage = self.quick_stage();
        let (_, icon) = workflow::status(stage);
        let belongs = move |k: &MenuKind| *k == kind;
        let open = self
            .menu
            .as_ref()
            .is_some_and(|m| belongs(&m.kind) && m.overlay.look(self.frame).open);
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id("quick-status")
                    .size(px(component::ICON_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::BUTTON))
                    .border(px(elevation::HAIRLINE))
                    .border_color(if self.workflow.quick.is_focused(window) {
                        p.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .track_focus(&self.workflow.quick)
                    .role(Role::Button)
                    .aria_label("Choose status")
                    .aria_expanded(open)
                    .tooltip(kit::tooltip(p, "Choose status"))
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .when(open || stage != QueryStatus::Todo, |s| s.bg(p.wash))
                    .child(kit::icon(
                        icon,
                        component::ICON,
                        status_ink(&p, stage, false),
                    ))
                    .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)))
                    .on_click(cx.listener(move |this, e, window, cx| {
                        this.toggle_menu(kind, Stop::QuickStatus, e, window, cx)
                    })),
            )
            .children(self.menu_anchor(belongs, cx))
    }

    pub(super) fn quick_status_chip(
        &self,
        stage: QueryStatus,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = self.palette;
        let (label, _) = workflow::status(stage);
        let name = format!("Remove status: {label}");
        div()
            .id("quick-clear-status")
            .track_focus(&self.workflow.clear)
            .role(Role::Button)
            .aria_label(name.clone())
            .tooltip(kit::tooltip(p, name))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S1))
            .h(px(space::S6))
            .rounded(px(radius::SM))
            .bg(p.wash)
            .border(px(elevation::HAIRLINE))
            .border_color(if self.workflow.clear.is_focused(window) {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .child(status_mark(&p, stage, false))
            .child(kit::icon(Icon::X, component::ICON_SMALL, p.ink_3))
            .on_click(cx.listener(|this, _, window, cx| {
                this.quick_pick(super::quick_pickers::Pick::Status(None), window, cx)
            }))
    }

    pub(super) fn setting_toggle(
        &self,
        item: &MenuItem,
        description: &'static str,
        focus: bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = self.palette;
        let on = item.checked;
        let action = item.action;
        div()
            .id(item.key.clone())
            .mx(px(space::S2))
            .p(px(space::S2))
            .flex()
            .items_start()
            .gap(px(space::S3))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .border_color(if focus {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .style(ty::NAV_SUB.with_weight(500))
                            .text_color(p.ink)
                            .child(item.label.clone()),
                    )
                    .child(div().style(ty::META).text_color(p.ink_3).child(description)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .when(on, |d| d.justify_end())
                    .w(px(desktop::TITLEBAR))
                    .h(px(component::CHIP))
                    .rounded_full()
                    .px(px(elevation::HAIRLINE + elevation::HAIRLINE))
                    .bg(if on { p.accent_fill } else { p.control })
                    .child(div().size(px(component::ICON)).rounded_full().bg(if on {
                        p.on_accent
                    } else {
                        p.raised
                    })),
            )
            .when(live, |d| {
                d.role(Role::Switch)
                    .aria_label(item.label.clone())
                    .aria_description(description)
                    .aria_toggled(if on {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(focus, |d| d.aria_active_descendant())
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let MenuAction::ListSetting(list, setting) = action {
                            this.apply_list_setting(list, setting, cx);
                        }
                    }))
            })
    }

    pub(super) fn default_view_choices(
        &self,
        items: &[MenuItem],
        focused: impl Fn(&MenuItem) -> bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        div()
            .px(px(space::S3))
            .py(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(
                div()
                    .style(ty::NAV_SUB.with_weight(500))
                    .child("Default view"),
            )
            .child(
                div()
                    .style(ty::META)
                    .text_color(p.ink_3)
                    .child("Opens this view when you visit the list."),
            )
            .child(
                div()
                    .id("default-view-tabs")
                    .role(Role::TabList)
                    .aria_label("Default view")
                    .flex()
                    .gap(px(space::S1))
                    .p(px(space::S1))
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line)
                    .bg(p.canvas)
                    .children(items.iter().map(|item| {
                        let action = item.action;
                        let focus = focused(item);
                        div()
                            .id(item.key.clone())
                            .role(Role::Tab)
                            .aria_label(item.label.clone())
                            .aria_selected(item.checked)
                            .when(focus, |d| d.aria_active_descendant())
                            .flex_1()
                            .min_w_0()
                            .h(px(component::VIEW_SWITCHER))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(space::S1))
                            .rounded(px(radius::SM))
                            .border(px(elevation::HAIRLINE))
                            .border_color(if focus || item.checked {
                                p.accent
                            } else {
                                gpui::transparent_black()
                            })
                            .when(item.checked, |d| d.bg(p.raised))
                            .style(ty::BUTTON_SMALL.with_weight(if item.checked {
                                600
                            } else {
                                500
                            }))
                            .text_color(if item.checked {
                                p.accent_strong
                            } else {
                                p.ink_2
                            })
                            .child(kit::icon(
                                item.icon,
                                component::ICON_SMALL,
                                if item.checked {
                                    p.accent_strong
                                } else {
                                    p.ink_3
                                },
                            ))
                            .child(item.label.clone())
                            .when(live, |d| {
                                d.cursor_pointer()
                                    .hover(|s| s.bg(p.hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let MenuAction::ListSetting(list, setting) = action {
                                            this.apply_list_setting(list, setting, cx);
                                        }
                                    }))
                            })
                    })),
            )
    }
}
