//! The desktop board canvas, navigation and pointer gesture. The shared client owns
//! grouping, the recent Done window and resolution of a visual slot into a command.

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use gpui::{
    AnyElement, Bounds, ClickEvent, Context, DispatchPhase, Div, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Role, ScrollHandle, SharedString, Window, canvas, div, point,
    prelude::*, px,
};
use todo_client::board::drop_command;
use todo_core::{
    Command,
    command::Placement,
    ids::{ListId, TaskId},
    model::ListView,
    query::QueryStatus,
};
use todo_design::{layout::board, motion};

use super::{
    RowLook, ShellView, component, desktop, elevation, kit, radius, space, ty, workflow::status_ink,
};
use crate::{
    model::{
        Due, RoomKey, TaskRow,
        motion::{Pace, Track},
        workflow::{self, Key, STAGES},
    },
    theme::{Icon, TextStyled},
};

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    column: usize,
    index: usize,
    card: Option<TaskId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    stage: QueryStatus,
    anchor: Option<(TaskId, bool)>,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    list: ListId,
    card: TaskId,
    start: Point<Pixels>,
    current: Point<Pixels>,
    origin: Bounds<Pixels>,
    lifted: bool,
    target: Option<Target>,
}

#[derive(Debug, Default)]
struct Geometry {
    column: [Option<Bounds<Pixels>>; 4],
    cards: HashMap<TaskId, Bounds<Pixels>>,
}

pub(super) struct BoardUi {
    pub view_override: Option<ListView>,
    pub composer: Option<QueryStatus>,
    cursor: Cursor,
    geometry: Rc<RefCell<Geometry>>,
    horizontally: ScrollHandle,
    columns: [ScrollHandle; 4],
    scrollbars: [kit::Scrollbar; 4],
    drag: Option<Drag>,
    swap: Track,
}

impl Default for BoardUi {
    fn default() -> Self {
        Self {
            view_override: None,
            composer: None,
            cursor: Cursor {
                column: QueryStatus::Todo as usize,
                ..Cursor::default()
            },
            geometry: Rc::default(),
            horizontally: ScrollHandle::new(),
            columns: std::array::from_fn(|_| ScrollHandle::new()),
            scrollbars: std::array::from_fn(|_| kit::Scrollbar::default()),
            drag: None,
            swap: Track::rest(1.),
        }
    }
}

impl BoardUi {
    pub fn reset_view(&mut self) {
        let view_override = self.view_override;
        *self = Self {
            view_override,
            ..Self::default()
        };
    }

    pub fn reset_room(&mut self) {
        *self = Self::default();
    }

    pub fn start_swap(&mut self, now: Duration, pace: Pace, animate: bool) {
        if animate {
            self.swap = Track::rest(0.);
            let ms = if pace.reduced {
                motion::REDUCED_FADE
            } else {
                motion::CONTENT_SWAP
            };
            self.swap
                .aim(1., now, Duration::from_millis(ms.into()), motion::STANDARD);
        }
    }

    pub fn opacity(&self, now: Duration) -> f32 {
        self.swap.at(now)
    }

    pub fn clear_geometry(&self) {
        *self.geometry.borrow_mut() = Geometry::default();
    }
}

impl ShellView {
    fn board_list(&self) -> Option<ListId> {
        match self.shell.room.key {
            RoomKey::List(id) if self.on_board() => Some(id),
            _ => None,
        }
    }

    fn done_collapsed(&self) -> bool {
        self.board_list().is_some_and(|list| {
            self.ui
                .board_done
                .get(&list)
                .copied()
                .unwrap_or(self.docked())
        })
    }

    fn toggle_done_rail(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.board_list() else {
            return;
        };
        let collapsed = !self.done_collapsed();
        self.ui.board_done.insert(list, collapsed);
        self.save_ui_preferences();
        cx.notify();
    }

    pub(super) fn aim_board_cursor(&mut self, id: TaskId) {
        let Some(board) = self.shell.room.board.as_ref() else {
            return;
        };
        if let Some((column, index)) =
            board
                .columns
                .iter()
                .enumerate()
                .find_map(|(column, cards)| {
                    cards
                        .iter()
                        .position(|c| c.id == id)
                        .map(|index| (column, index))
                })
        {
            self.board.cursor = Cursor {
                column,
                index,
                card: Some(id),
            };
        }
    }

    fn cursor_position(&self) -> (usize, usize) {
        let cursor = self.board.cursor;
        let Some(board) = &self.shell.room.board else {
            return (cursor.column, cursor.index);
        };
        cursor
            .card
            .and_then(|id| {
                board.columns.iter().enumerate().find_map(|(col, cards)| {
                    cards.iter().position(|c| c.id == id).map(|row| (col, row))
                })
            })
            .unwrap_or((cursor.column, cursor.index))
    }

    pub(super) fn board_nav_ids(&self) -> Vec<TaskId> {
        self.shell
            .room
            .board
            .iter()
            .flat_map(|board| board.columns.iter().enumerate())
            .filter(|(i, _)| *i != QueryStatus::Done as usize || !self.done_collapsed())
            .flat_map(|(_, rows)| rows.iter().map(|row| row.id))
            .collect()
    }

    pub(super) fn board_task(&self) -> Option<TaskId> {
        let board = self.shell.room.board.as_ref()?;
        let (column, index) = self.cursor_position();
        if column == QueryStatus::Done as usize && self.done_collapsed() {
            return None;
        }
        let rows = &board.columns[column];
        rows.get(index.min(rows.len().saturating_sub(1)))
            .map(|row| row.id)
    }

    pub(super) fn board_anchor(&self, id: TaskId, window: &Window) -> Point<Pixels> {
        self.board
            .geometry
            .borrow()
            .cards
            .get(&id)
            .map_or_else(|| self.row_anchor(id, window), |b| b.bottom_left())
    }

    pub(super) fn begin_column_add(
        &mut self,
        stage: QueryStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if workflow::open_stage(stage).is_none() || !self.on_board() {
            return;
        }
        self.board.composer = Some(stage);
        self.board.cursor = Cursor {
            column: stage as usize,
            ..Cursor::default()
        };
        self.reveal_board(self.board.cursor.column, None);
        window.focus(&self.quick_focus.input, cx);
        cx.notify();
    }

    pub(super) fn board_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (column, _) = self.cursor_position();
        self.begin_column_add(
            if STAGES[column] == QueryStatus::Done {
                QueryStatus::Todo
            } else {
                STAGES[column]
            },
            window,
            cx,
        );
    }

    pub(super) fn board_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.board_list() else {
            return;
        };
        let task = self.board_task();
        let (column, index) = self.cursor_position();
        let Some(board) = self.shell.room.board.as_ref() else {
            return;
        };
        match key {
            Key::Column(by) | Key::Row(by) => {
                let (mut col, mut row) = (column, index);
                match key {
                    Key::Column(_) => {
                        col = (col as isize + by).clamp(0, STAGES.len() as isize - 1) as usize
                    }
                    _ => row = (row as isize + by).max(0) as usize,
                }
                let cards = &board.columns[col];
                row = row.min(cards.len().saturating_sub(1));
                let card = cards.get(row).map(|row| row.id);
                self.board.cursor = Cursor {
                    column: col,
                    index: row,
                    card,
                };
                self.reveal_board(col, card);
                cx.notify();
            }
            Key::MoveColumn(by) => {
                if let Some(card) = task
                    && let Some(target) = column
                        .checked_add_signed(by)
                        .and_then(|i| STAGES.get(i))
                        .copied()
                {
                    let anchor = board.columns[target as usize]
                        .get(index)
                        .map(|row| (row.id, false));
                    self.move_board_card(list, card, target, workflow::slot(anchor), cx);
                    self.aim_board_cursor(card);
                    self.reveal_board(target as usize, Some(card));
                }
            }
            Key::MoveRow(by) => {
                if let Some(card) = task
                    && let Some(anchor) = index
                        .checked_add_signed(by)
                        .and_then(|i| board.columns[column].get(i))
                {
                    self.move_board_card(
                        list,
                        card,
                        STAGES[column],
                        workflow::slot(Some((anchor.id, by > 0))),
                        cx,
                    );
                    self.aim_board_cursor(card);
                    self.reveal_board(column, Some(card));
                }
            }
            Key::Add => self.board_add(window, cx),
            Key::Open if STAGES[column] == QueryStatus::Done && self.done_collapsed() => {
                self.toggle_done_rail(cx)
            }
            Key::Open | Key::Edit | Key::Due => {
                if let Some(task) = task {
                    self.select_task(Some(task), cx);
                    if self.ui.task == Some(task) {
                        if key == Key::Edit {
                            let focus = self.edit.title.read(cx).focus_handle.clone();
                            self.edit.title.update(cx, |i, cx| i.select_everything(cx));
                            window.focus(&focus, cx);
                        } else if key == Key::Due {
                            self.start_due(window, cx);
                        }
                    }
                }
            }
            Key::Complete => {
                if let Some(task) = task {
                    self.toggle_complete(task, cx);
                }
            }
            Key::Escape => {
                if self.board.drag.take().is_some() || self.board.composer.take().is_some() {
                    cx.notify();
                } else {
                    self.select_task(None, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_board_card(
        &mut self,
        list: ListId,
        card: TaskId,
        stage: QueryStatus,
        slot: Placement,
        cx: &mut Context<Self>,
    ) {
        if !self.commit_edits(cx) {
            return self.keep_unsaved(cx);
        }
        let state = self.store.view();
        let Some(cmd) = drop_command(&state, list, card, stage, slot) else {
            return;
        };
        let completion = matches!(cmd, Command::CompleteTask { .. });
        let t = state.tasks.get(&card);
        let same = cmd.clone();
        if self.submit_if(cmd, |live| {
            drop_command(live, list, card, stage, slot).as_ref() == Some(&same)
        }) == Ok(true)
            && let Some(task) = t.filter(|task| completion || task.is_done())
        {
            self.play_completion(task);
        }
        self.refresh(cx);
    }

    fn reveal_board(&self, column: usize, card: Option<TaskId>) {
        let geometry = self.board.geometry.borrow();
        if let Some(bounds) = geometry.column[column] {
            let h = &self.board.horizontally;
            let view = h.bounds();
            let x = if bounds.left() < view.left() {
                view.left() - bounds.left()
            } else if bounds.right() > view.right() {
                view.right() - bounds.right()
            } else {
                px(0.)
            };
            let old = h.offset();
            h.set_offset(point((old.x + x).clamp(-h.max_offset().x, px(0.)), old.y));
        }
        if let Some(bounds) = card.and_then(|id| geometry.cards.get(&id).copied()) {
            self.reveal_bounds(&self.board.columns[column], bounds);
        }
    }

    fn board_target(&self, drag: Drag) -> Option<Target> {
        let stage = &self.board.horizontally;
        if self.board_list() != Some(drag.list)
            || !stage.bounds().contains(&drag.current)
            || (self.width == crate::model::layout::Width::Narrow && self.shell.detail.is_some())
        {
            return None;
        }
        let board = self.shell.room.board.as_ref()?;
        let geometry = self.board.geometry.borrow();
        let i = geometry
            .column
            .iter()
            .position(|b| b.is_some_and(|b| b.contains(&drag.current)))?;
        let target = STAGES[i];
        let scroll = &self.board.columns[i];
        let anchor = if target == QueryStatus::Done || !scroll.bounds().contains(&drag.current) {
            None
        } else {
            let mut last = None;
            for row in &board.columns[i] {
                if row.id == drag.card {
                    continue;
                }
                if let Some(bounds) = geometry.cards.get(&row.id) {
                    if bounds.contains(&drag.current) {
                        last = Some((row.id, drag.current.y >= bounds.center().y));
                        break;
                    }
                    if drag.current.y < bounds.center().y {
                        last = Some((row.id, false));
                        break;
                    }
                    last = Some((row.id, true));
                }
            }
            last
        };
        drop_command(
            &self.store.view(),
            drag.list,
            drag.card,
            target,
            workflow::slot(anchor),
        )?;
        Some(Target {
            stage: target,
            anchor,
        })
    }

    pub(super) fn board_composer_open(&self) -> bool {
        self.on_board() && self.board.composer.is_some()
    }

    pub(super) fn drive_board(&mut self, window: &mut Window) {
        if self.board.swap.moving(self.frame) {
            window.request_animation_frame();
        }
        let Some(drag) = self.board.drag.filter(|g| g.lifted) else {
            return;
        };
        if !window.is_window_active()
            || self.board_list() != Some(drag.list)
            || self.task_stage(drag.card).is_none()
        {
            self.board.drag = None;
            return;
        }
        scroll_edge(&self.board.horizontally, drag.current, true, window);
        let target = self.board_target(drag);
        if let Some(target) = target {
            scroll_edge(
                &self.board.columns[target.stage as usize],
                drag.current,
                false,
                window,
            );
        }
        if let Some(g) = &mut self.board.drag {
            g.target = target;
        }
    }

    pub(super) fn board_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let moved = view.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    moved
                        .update(cx, |this, cx| {
                            let Some(g) = &mut this.board.drag else {
                                return;
                            };
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.board.drag = None;
                                cx.notify();
                                return;
                            }
                            g.current = e.position;
                            let d = (g.current - g.start).map(f32::from);
                            g.lifted |= d.x.abs().max(d.y.abs()) >= space::S1;
                            if g.lifted {
                                cx.notify();
                                cx.stop_propagation();
                            }
                        })
                        .ok();
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        let Some(mut g) = this.board.drag.take().filter(|g| g.lifted) else {
                            return;
                        };
                        g.current = e.position;
                        if let Some(target) = this.board_target(g) {
                            this.move_board_card(
                                g.list,
                                g.card,
                                target.stage,
                                workflow::slot(target.anchor),
                                cx,
                            );
                            this.aim_board_cursor(g.card);
                        }
                        window.focus(&this.list_focus, cx);
                        cx.stop_propagation();
                        cx.notify();
                    })
                    .ok();
                });
            },
        )
        .absolute()
        .size_0()
    }

    pub(super) fn board_preview(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let g = self.board.drag.filter(|g| g.lifted)?;
        let row = self
            .shell
            .room
            .board
            .as_ref()?
            .columns
            .iter()
            .flatten()
            .find(|r| r.id == g.card)?;
        let origin = g.origin.origin + g.current - g.start;
        Some(
            div()
                .absolute()
                .top(origin.y)
                .left(origin.x)
                .w(g.origin.size.width)
                .child(self.board_card(row, None, g.origin.size.width.into(), false, window, cx)),
        )
    }

    pub(super) fn board_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let Some(data) = &self.shell.room.board else {
            return div();
        };
        let collapsed = self.done_collapsed();
        let docked = self.docked() && self.shell.detail.is_some();
        let width = column_width(self.room_width(window), collapsed, docked);
        let focused = self
            .list_focus
            .is_focused(window)
            .then(|| self.board_task())
            .flatten();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .opacity(self.board.opacity(self.frame))
            .child(
                div()
                    .id("board")
                    .track_focus(&self.list_focus)
                    .size_full()
                    .flex()
                    .key_context("TaskBoard")
                    .on_action(cx.listener(|this, _: &super::SelectAllTasks, _, cx| {
                        this.change_selection(todo_client::selection::Selection::all, cx);
                    }))
                    .on_action(cx.listener(|this, _: &super::ExtendTasksUp, _, cx| {
                        let previous = this.board_task();
                        this.change_selection(|s, rows, _| s.extend(rows, -1, previous), cx);
                    }))
                    .on_action(cx.listener(|this, _: &super::ExtendTasksDown, _, cx| {
                        let previous = this.board_task();
                        this.change_selection(|s, rows, _| s.extend(rows, 1, previous), cx);
                    }))
                    .overflow_x_scroll()
                    .track_scroll(&self.board.horizontally)
                    .on_key_down(cx.listener(|this, e, window, cx| {
                        if this.workflow_key(e, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .flex_none()
                            .items_stretch()
                            .gap(px(board::COL_GAP))
                            .px(px(desktop::LIST_PAD_X))
                            .pt(px(space::S4))
                            .pb(px(space::S3))
                            .children(STAGES.into_iter().zip(&data.columns).map(
                                |(stage, cards)| {
                                    self.board_column(
                                        stage,
                                        cards,
                                        width,
                                        data.total_done,
                                        collapsed,
                                        focused,
                                        window,
                                        cx,
                                    )
                                },
                            )),
                    ),
            )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one board column with its layout and live controls"
    )]
    fn board_column(
        &self,
        stage: QueryStatus,
        rows: &[TaskRow],
        width: f32,
        total_done: usize,
        collapsed: bool,
        focused: Option<TaskId>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let i = stage as usize;
        let done = stage == QueryStatus::Done;
        let rail = done && collapsed;
        let (name, icon) = workflow::status(stage);
        let target = self
            .board
            .drag
            .and_then(|g| g.target)
            .filter(|target| target.stage == stage);
        let keyboard = self.list_focus.is_focused(window) && self.cursor_position().0 == i;
        let geometry = self.board.geometry.clone();
        let probe = canvas(
            |bounds, _, _| bounds,
            move |_, bounds, _, _| geometry.borrow_mut().column[i] = Some(bounds),
        )
        .absolute()
        .inset_0();
        let well = div()
            .id(("board-column", i))
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if rail { board::COL_COLLAPSED } else { width }))
            .min_h_0()
            .rounded(px(radius::LG))
            .border(px(elevation::HAIRLINE))
            .border_color(if target.is_some() && rail || keyboard && rows.is_empty() {
                p.accent
            } else {
                p.line
            })
            .bg(if rail && target.is_some() {
                p.wash
            } else {
                p.canvas
            })
            .child(probe);
        if rail {
            return well
                .items_center()
                .gap(px(space::S2))
                .py(px(space::S3))
                .role(Role::Button)
                .aria_label(format!("Expand Done: {} recent completions", rows.len()))
                .aria_expanded(false)
                .tooltip(kit::tooltip(p, "Expand Done"))
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(kit::icon(icon, component::ICON, p.accent))
                .child(
                    div()
                        .style(ty::BUTTON_SMALL)
                        .text_color(p.ink_2)
                        .child(name),
                )
                .child(
                    div()
                        .style(ty::COUNT)
                        .text_color(p.ink_3)
                        .child(rows.len().to_string()),
                )
                .child(kit::icon(Icon::ChevronLeft, component::ICON_SMALL, p.ink_3))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_done_rail(cx)))
                .into_any_element();
        }
        let stage_ink = status_ink(&p, stage, false);
        let mut body = div()
            .id(("board-stack", i))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.board.columns[i])
            .gap(px(board::CARD_GAP))
            .p(px(space::S2))
            .children(rows.iter().map(|row| {
                let preview = self
                    .board
                    .drag
                    .is_some_and(|g| g.lifted && g.card == row.id);
                let card_width = width - 2. * (space::S2 + elevation::HAIRLINE);
                self.board_card(
                    row,
                    Some((stage, focused == Some(row.id))),
                    card_width,
                    preview,
                    window,
                    cx,
                )
            }));
        if !done && (rows.is_empty() || stage == QueryStatus::Backlog) {
            let line = match stage {
                QueryStatus::Backlog => "Drop future ideas here",
                QueryStatus::Todo => "Nothing to do yet",
                _ => "Nothing in progress",
            };
            body = body.child(
                div()
                    .flex_none()
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_dashed()
                    .border_color(if target.is_some() && rows.is_empty() {
                        p.accent_strong
                    } else {
                        p.line_strong
                    })
                    .when(target.is_some() && rows.is_empty(), |d| d.bg(p.wash))
                    .py(px(space::S4))
                    .px(px(space::S2))
                    .text_center()
                    .style(ty::META)
                    .text_color(p.ink_3)
                    .child(line),
            );
        }
        if target.is_some_and(|t| t.anchor.is_none()) {
            body = body.child(drop_line(&p));
        }
        let heading = div()
            .id("column-heading")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .role(Role::Heading)
            .aria_label(format!("{name}, {} tasks", rows.len()))
            .child(kit::icon(icon, component::ICON, stage_ink))
            .child(
                div()
                    .truncate()
                    .style(ty::NAV_SUB.with_weight(600))
                    .text_color(if stage == QueryStatus::InProgress {
                        stage_ink
                    } else {
                        p.ink
                    })
                    .child(name),
            );
        well.child(
            div()
                .h(px(board::HEADER_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .border_b(px(elevation::HAIRLINE))
                .border_color(p.line)
                .child(heading)
                .child(
                    div()
                        .style(ty::COUNT)
                        .text_color(if stage == QueryStatus::InProgress {
                            p.accent_strong
                        } else {
                            p.ink_3
                        })
                        .child(if done {
                            format!("{} recent", rows.len())
                        } else {
                            rows.len().to_string()
                        }),
                )
                .when(done, |d| {
                    d.child(
                        div()
                            .id("collapse-done")
                            .role(Role::Button)
                            .aria_label("Collapse Done")
                            .aria_expanded(true)
                            .tooltip(kit::tooltip(p, "Collapse Done"))
                            .cursor_pointer()
                            .child(kit::icon(
                                Icon::ChevronRight,
                                component::ICON_SMALL,
                                p.ink_3,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_done_rail(cx))),
                    )
                }),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(body)
                .child(self.board.scrollbars[i].overlay(&self.board.columns[i], &p, self.animate)),
        )
        .when(!done, |well| {
            well.child(self.column_composer(stage, window, cx))
        })
        .when(done && total_done > 0, |well| {
            well.child(
                div()
                    .id("all-done")
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .style(ty::BUTTON_SMALL)
                    .text_color(p.accent_strong)
                    .cursor_pointer()
                    .role(Role::Button)
                    .hover(|s| s.bg(p.hover))
                    .child(format!("View all {total_done} in Completed →"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_board_completed(window, cx)),
                    ),
            )
        })
        .into_any_element()
    }

    fn show_board_completed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commit_edits(cx) {
            return self.keep_unsaved(cx);
        }
        self.select_task(None, cx);
        self.switch_view(ListView::List, window, cx);
        self.ui.show_completed = true;
        self.refresh(cx);
        let i = self.shell.room.tasks.len();
        if i < self.shell.row_count() {
            self.rows.scroll_to_item(i, gpui::ScrollStrategy::Center);
        }
    }

    fn column_composer(&self, stage: QueryStatus, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let (name, _) = workflow::status(stage);
        let label = format!("Add to {name}");
        let active = self.board.composer == Some(stage);
        let footer = div()
            .mx(px(space::S2))
            .mb(px(space::S2))
            .flex_none()
            .rounded(px(radius::MD));
        if active {
            let draft = self.quick_preview();
            return footer
                .bg(p.raised)
                .border(px(elevation::HAIRLINE))
                .border_color(p.accent)
                .child(
                    div()
                        .h(px(board::HEADER_HEIGHT))
                        .px(px(space::S2))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .style(ty::BODY)
                                .child(self.quick_add.clone()),
                        )
                        .child(self.quick_picker_button(
                            true,
                            draft.due.is_some() || draft.start_date.is_some(),
                            window,
                            cx,
                        ))
                        .child(
                            self.quick_picker_button(
                                false,
                                draft
                                    .priority
                                    .is_some_and(|p| p != todo_core::model::Priority::None),
                                window,
                                cx,
                            ),
                        ),
                )
                .when(
                    self.quick_date_label(&draft).is_some()
                        || draft
                            .priority
                            .is_some_and(|p| p != todo_core::model::Priority::None),
                    |d| {
                        d.child(
                            div()
                                .px(px(space::S2))
                                .pb(px(space::S1))
                                .flex()
                                .gap(px(space::S1))
                                .children(
                                    self.quick_date_label(&draft)
                                        .map(|label| self.quick_date_chip(label, window, cx)),
                                )
                                .children(
                                    draft
                                        .priority
                                        .filter(|&p| p != todo_core::model::Priority::None)
                                        .map(|p| self.quick_priority_chip(p, window, cx)),
                                ),
                        )
                    },
                );
        }
        footer.child(
            div()
                .id(("add-column", stage as usize))
                .h(px(board::HEADER_HEIGHT))
                .px(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .rounded(px(radius::MD))
                .role(Role::Button)
                .aria_label(label.clone())
                .cursor_pointer()
                .hover(|s| s.bg(p.hover))
                .style(ty::NAV_SUB)
                .text_color(p.ink_3)
                .child(kit::icon(
                    Icon::Plus,
                    component::ICON_SMALL,
                    p.accent_strong,
                ))
                .child(label)
                .on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.begin_column_add(stage, window, cx)
                    }),
                ),
        )
    }

    fn board_card(
        &self,
        row: &TaskRow,
        stage: Option<(QueryStatus, bool)>,
        width: f32,
        placeholder: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let id = row.id;
        let lifted = stage.is_none();
        let bulk = stage.is_some() && self.multi.contains(id);
        let p = self.palette.for_task_row(bulk);
        let cursor = stage.is_some_and(|(_, focused)| focused);
        let selected = self.ui.task == Some(id) || (cursor && self.ui.task.is_none() && !bulk);
        let look = self
            .motion
            .row(id, self.frame)
            .unwrap_or(RowLook::rest(row.done));
        let overdue = matches!(row.due, Due::Overdue(_));
        let check = kit::checkbox_at(
            &p,
            overdue,
            selected || bulk,
            component::CHECKBOX,
            (look.fill, look.scale, look.tick),
        );
        let star = row.priority == 3;
        let text = div()
            .flex_1()
            .min_w_0()
            .line_clamp(2)
            .text_ellipsis()
            .style(ty::BODY.with_weight(500))
            .text_color(super::theme::mix(p.ink, p.ink_3, look.strike))
            .when(look.strike > 0., |d| d.line_through())
            .child(row.title.clone());
        let layout = div()
            .flex()
            .items_start()
            .gap(px(space::S2))
            .child(
                div()
                    .id("card-check")
                    .flex_none()
                    .child(check)
                    .when(stage.is_some(), |d| {
                        d.cursor_pointer()
                            .role(Role::Button)
                            .aria_label(if row.done {
                                "Reopen task"
                            } else {
                                "Complete task"
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                if !this.modified_task_click(id, e.modifiers(), cx) {
                                    this.toggle_complete(id, cx);
                                }
                            }))
                    }),
            )
            .child(text)
            .when(!row.done && (star || stage.is_some()), |d| {
                d.child(
                    div()
                        .id("card-star")
                        .flex_none()
                        .opacity(if star || cursor { 1. } else { 0. })
                        .group_hover("board-card", |s| s.opacity(1.))
                        .child(self.task_glyph(&p, id, Icon::Star, star, component::ICON_SMALL))
                        .when(stage.is_some(), |d| {
                            d.cursor_pointer()
                                .role(Role::Button)
                                .aria_label("Important")
                                .aria_toggled(if star {
                                    gpui::Toggled::True
                                } else {
                                    gpui::Toggled::False
                                })
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_task_glyph(id, Icon::Star, cx);
                                }))
                        }),
                )
            });
        let meta = self.board_metadata(row, &p, width, window);
        let mut card = div()
            .id(SharedString::from(format!("card-{id}")))
            .group("board-card")
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(board::CARD_GAP))
            .p(px(board::CARD_PAD))
            .bg(if selected || placeholder {
                p.wash
            } else if bulk {
                p.selection_wash
            } else {
                p.raised
            })
            .rounded(px(radius::LG))
            .border(px(elevation::HAIRLINE))
            .border_color(if cursor || selected || lifted {
                p.accent
            } else {
                p.line
            })
            .when(lifted, |d| d.shadow(kit::overlay_shadow(&p)))
            .child(layout.opacity(if placeholder { 0. } else { 1. }))
            .children(meta.map(|m| m.opacity(if placeholder { 0. } else { 1. })))
            .when((selected || bulk) && !placeholder, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(space::S2))
                        .bottom(px(space::S2))
                        .w(px(elevation::SELECTION_BAR))
                        .rounded(px(radius::SM))
                        .bg(p.accent_strong),
                )
            });
        if let (Some((stage, _)), Some(list)) = (stage, self.board_list()) {
            let geometry = self.board.geometry.clone();
            let dragging = self
                .board
                .drag
                .and_then(|g| g.target)
                .and_then(|t| t.anchor)
                .filter(|(anchor, _)| *anchor == id);
            card = card
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(row.title.clone())
                .aria_selected(selected || bulk)
                .when(cursor, |d| d.aria_active_descendant())
                .when(!selected && !bulk && !placeholder, |d| {
                    d.hover(|s| s.border_color(p.line_strong))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, window, _| {
                        if e.modifiers.modified() || window.default_prevented() {
                            return;
                        }
                        let origin = this.board.geometry.borrow().cards.get(&id).copied();
                        if let Some(origin) = origin
                            && this.task_stage(id) == Some(stage)
                        {
                            this.board.drag = Some(Drag {
                                list,
                                card: id,
                                start: e.position,
                                current: e.position,
                                origin,
                                lifted: false,
                                target: None,
                            });
                        }
                    }),
                )
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    window.focus(&this.list_focus, cx);
                    this.aim_board_cursor(id);
                    this.click_task(id, e.modifiers(), cx);
                }))
                .child(
                    canvas(
                        |b, _, _| b,
                        move |_, b, _, _| {
                            geometry.borrow_mut().cards.insert(id, b);
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .when_some(dragging, |d, (_, after)| {
                    d.child(
                        drop_line(&p)
                            .absolute()
                            .left_0()
                            .right_0()
                            .when(after, |d| d.bottom(px(-board::CARD_GAP / 2.)))
                            .when(!after, |d| d.top(px(-board::CARD_GAP / 2.))),
                    )
                });
        }
        div()
            .flex_none()
            .when(!lifted && look.height < 1., |d| {
                d.overflow_hidden().max_h(px((board::CARD_PAD * 2.
                    + ty::QUICK_ADD.line_height * 2.
                    + component::CHIP
                    + board::CARD_GAP)
                    * look.height))
            })
            .opacity(if lifted { 1. } else { look.opacity })
            .child(card)
    }

    fn board_metadata(
        &self,
        row: &TaskRow,
        p: &crate::theme::Palette,
        width: f32,
        window: &Window,
    ) -> Option<Div> {
        use crate::model::fit;
        let gap = component::CHIP_GAP;
        let indent = component::CHECKBOX + space::S2;
        let steps = row
            .steps
            .filter(|_| !row.done)
            .map(|(d, n)| (d, n, format!("{d}/{n}")));
        let size = |style, text: &str| kit::text_width(window, style, text).ceil();
        let card_room = width
            - 2. * (board::CARD_PAD + elevation::HAIRLINE)
            - indent
            - row.agent.map_or(0., |_| component::CHIP + space::S1);
        let date = if row.done {
            row.completed_when.as_ref()
        } else {
            row.due.text()
        };
        let date_width = date.map_or(0., |date| {
            if matches!(row.due, Due::Overdue(_)) {
                size(ty::OVERDUE_BADGE, date)
                    + component::CHIP_DOT
                    + elevation::HAIRLINE
                    + component::CHIP_GAP
                    + 2. * component::CHIP_PAD_X
            } else {
                size(ty::DUE.with_weight(600), date)
            }
        });
        let priority = if row.done { 0 } else { row.priority };
        let priority_width = if priority == 0 {
            0.
        } else {
            size(ty::PRIORITY, "!") * f32::from(priority) + gap
        };
        let items = fit::MetaItems {
            gap,
            chip: None,
            steps: steps
                .as_ref()
                .map(|(_, n, count)| (kit::meter_width(*n), space::S1, size(ty::COUNT, count))),
            mail: (!row.done && row.from_mail).then_some(component::ICON_SMALL),
            // Give at most two tags their measured widths. The remainder can only appear as +N.
            tags: row
                .tags
                .iter()
                .enumerate()
                .map(|(i, tag)| {
                    if i < 2 {
                        size(ty::TAG, tag)
                    } else {
                        f32::INFINITY
                    }
                })
                .collect(),
        };
        let available = card_room - date.map_or(0., |_| date_width + gap) - priority_width;
        let fit = fit::fit_meta(available, &items, |n| size(ty::TAG, &fit::more_label(n)));
        let tag = |label| {
            div()
                .flex_none()
                .style(ty::TAG)
                .text_color(p.ink_3)
                .child(label)
        };
        let has_fields = date.is_some()
            || steps.is_some()
            || priority > 0
            || row.from_mail
            || !row.tags.is_empty()
            || row.agent.is_some();
        has_fields.then(|| {
            div()
                .pl(px(indent))
                .flex()
                .items_center()
                .gap(px(space::S1))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(gap))
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .when_some(date, |el, date| {
                            if row.done {
                                el.child(
                                    div()
                                        .flex_none()
                                        .style(ty::DUE)
                                        .text_color(p.ink_3)
                                        .child(date.clone()),
                                )
                            } else {
                                el.child(Self::due(p, &row.due))
                            }
                        })
                        .when_some(
                            steps.filter(|_| fit.meter || fit.count),
                            |el, (d, n, label)| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(space::S1))
                                        .text_color(p.ink_3)
                                        .style(ty::COUNT)
                                        .when(fit.meter, |el| el.child(kit::meter(p, d, n)))
                                        .when(fit.count, |el| el.child(label)),
                                )
                            },
                        )
                        .when(fit.mail, |el| {
                            el.child(kit::icon(Icon::Mail, component::ICON_SMALL, p.ink_3))
                        })
                        .when(priority > 0, |el| el.child(kit::priority(p, priority)))
                        .children(row.tags[..fit.tags.shown].iter().cloned().map(tag))
                        .when(fit.tags.more > 0, |el| {
                            el.child(tag(fit::more_label(fit.tags.more).into()))
                        }),
                )
                .children(
                    row.agent
                        .map(|agent| kit::agent_mark(p.theme, agent, component::CHIP)),
                )
        })
    }
}

fn drop_line(p: &crate::theme::Palette) -> Div {
    div()
        .h(px(elevation::SELECTION_BAR))
        .rounded(px(radius::SM))
        .flex_none()
        .bg(p.accent_strong)
}

fn column_width(room: f32, rail: bool, beside_detail: bool) -> f32 {
    let width = room
        - 2. * desktop::LIST_PAD_X
        - 3. * board::COL_GAP
        - if rail { board::COL_COLLAPSED } else { 0. };
    let n = if rail { 3. } else { 4. };
    // Keep three open stages visible beside detail at reference width. A smaller pane scrolls.
    let min = if rail && beside_detail {
        desktop::SIDEBAR_MIN
    } else {
        board::COL_MIN
    };
    (width / n).clamp(min, board::COL_MAX)
}

fn scroll_edge(
    handle: &ScrollHandle,
    pointer: Point<Pixels>,
    horizontal: bool,
    window: &mut Window,
) {
    let view = handle.bounds();
    if !view.contains(&pointer) {
        return;
    }
    let old = handle.offset();
    let max = handle.max_offset();
    let strength = |gap: Pixels| (1. - f32::from(gap) / space::S8).clamp(0., 1.);
    let movement = |before, after| px((strength(before) - strength(after)) * space::S3);
    let next = if horizontal {
        point(
            (old.x + movement(pointer.x - view.left(), view.right() - pointer.x))
                .clamp(-max.x, px(0.)),
            old.y,
        )
    } else {
        point(
            old.x,
            (old.y + movement(pointer.y - view.top(), view.bottom() - pointer.y))
                .clamp(-max.y, px(0.)),
        )
    };
    if old != next {
        handle.set_offset(next);
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_board(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<ShellView>, &mut gpui::VisualTestContext) {
        use crate::{fixtures, store::Store, theme::Theme};
        cx.update(|cx| {
            cx.bind_keys(super::super::key_bindings());
            cx.bind_keys(crate::ui::input::key_bindings());
        });
        let f = fixtures::by_name("board").expect("board fixture");
        cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                Rc::new(Store::fixture(&f)),
                f.ui,
                Theme::Light,
                None,
                window,
                cx,
            );
            view.still();
            window.activate_window();
            window.focus(&view.list_focus, cx);
            view
        })
    }

    #[gpui::test]
    fn board_keys_move_reorder_undo_and_stage_a_column_draft(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture_board(cx);
        let card = cx.update(|_, cx| shell.read(cx).board_task().expect("first To do card"));
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(v.task_stage(card), Some(QueryStatus::Backlog));
            assert_eq!(v.board_task(), Some(card));
            assert!(v.last_undo.is_some());
        });
        cx.simulate_keystrokes("alt-down");
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(v.cursor_position(), (0, 1));
        });
        cx.simulate_keystrokes("ctrl-z h n o a k enter");
        cx.update(|_, cx| {
            let v = shell.read(cx);
            let state = v.store.view();
            let new = state
                .tasks
                .values()
                .find(|t| t.title == "oak")
                .expect("new task");
            assert_eq!(QueryStatus::of(new), QueryStatus::Backlog);
        });
        cx.simulate_keystrokes("escape 1 3");
        cx.update(|_, cx| assert_eq!(shell.read(cx).view(), ListView::List));
        cx.simulate_keystrokes("v");
        cx.update(|_, cx| assert_eq!(shell.read(cx).view(), ListView::Board));
    }

    #[gpui::test]
    fn pointer_drop_after_a_card_uses_its_column_slot_and_one_undo(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture_board(cx);
        let mods = gpui::Modifiers::default();
        cx.simulate_mouse_move(point(px(0.), px(0.)), None, mods);
        let (card, anchor, from, to) = cx.update(|_, cx| {
            let v = shell.read(cx);
            let board = v.shell.room.board.as_ref().expect("board");
            let (card, anchor) = (board.columns[0][0].id, board.columns[1][0].id);
            let g = v.board.geometry.borrow();
            let source = g.cards[&card].center();
            let anchor_bounds = g.cards[&anchor];
            (
                card,
                anchor,
                source,
                point(
                    anchor_bounds.center().x,
                    anchor_bounds.center().y + px(space::S1),
                ),
            )
        });
        cx.simulate_mouse_move(from, None, mods);
        cx.simulate_mouse_down(from, MouseButton::Left, mods);
        cx.simulate_mouse_move(to, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(
                v.board.drag.and_then(|g| g.target),
                Some(Target {
                    stage: QueryStatus::Todo,
                    anchor: Some((anchor, true))
                })
            );
        });
        cx.simulate_mouse_up(to, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            let todo = &v.shell.room.board.as_ref().expect("board").columns[1];
            assert_eq!((todo[0].id, todo[1].id), (anchor, card));
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(shell.read(cx).task_stage(card), Some(QueryStatus::Backlog)));
    }

    #[gpui::test]
    fn status_and_starts_have_real_list_keyboard_paths_and_undo(cx: &mut gpui::TestAppContext) {
        use crate::{fixtures, store::Store, theme::Theme};
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let f = fixtures::by_name("list-status").expect("list status fixture");
        let task = f.ui.task.expect("selected task");
        let original = f.state.tasks[&task].clone();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                Rc::new(Store::fixture(&f)),
                f.ui,
                Theme::Light,
                None,
                window,
                cx,
            );
            view.still();
            window.focus(&view.list_focus, cx);
            view
        });
        cx.simulate_keystrokes("s");
        cx.update(|_, cx| assert!(matches!(shell.read(cx).menu.as_ref().map(|m| m.kind), Some(super::super::MenuKind::Status { task: Some(id), .. }) if id == task)));
        cx.simulate_keystrokes("escape shift-s");
        cx.update(|_, cx| assert_eq!(shell.read(cx).task_stage(task), Some(QueryStatus::Done)));
        cx.simulate_keystrokes("ctrl-z shift-d");
        cx.update(|_, cx| assert!(matches!(shell.read(cx).menu.as_ref().map(|m| m.kind), Some(super::super::MenuKind::Date(c)) if c.starts)));
        cx.simulate_keystrokes("down enter");
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert!(state.tasks[&task].start_date > original.start_date);
            assert_eq!(state.tasks[&task].start_date, state.tasks[&task].due);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert_eq!(state.tasks[&task].start_date, original.start_date);
            assert_eq!(state.tasks[&task].due, original.due);
        });
    }

    #[gpui::test]
    fn completed_recent_card_can_be_reopened_in_a_column_and_undone(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture_board(cx);
        cx.simulate_keystrokes("l l");
        let card = cx.update(|_, cx| {
            let view = shell.read(cx);
            let card = view.board_task().expect("recent Done card");
            assert_eq!(view.task_stage(card), Some(QueryStatus::Done));
            card
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).task_stage(card),
                Some(QueryStatus::InProgress)
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(shell.read(cx).task_stage(card), Some(QueryStatus::Done)));
    }

    #[test]
    fn reference_width_fits_four_wells_or_three_wells_and_a_rail() {
        for (room, rail, count) in [(1192., false, 4.), (799., true, 3.)] {
            let rails = if rail { board::COL_COLLAPSED } else { 0. };
            assert!(
                count * column_width(room, rail, rail)
                    + rails
                    + 2. * desktop::LIST_PAD_X
                    + 3. * board::COL_GAP
                    <= room + f32::EPSILON
            );
        }
        assert_eq!(column_width(550., false, false), board::COL_MIN);
    }
}
