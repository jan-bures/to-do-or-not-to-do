//! The sticky task rail, calendar ruler and scheduling gestures. The grid and bars use the same
//! date axis, so resizing, panning, dropping and keyboard edits cannot disagree about a day.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    Bounds, Context, DispatchPhase, Div, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Role, ScrollHandle, SharedString, Stateful,
    Window, canvas, div, fill, point, prelude::*, px, size,
};
use todo_client::{
    intent::workflow::{self, Dates, Schedule},
    timeline::{Lane, Span, SpanKind, Zoom},
};
use todo_core::{
    ids::TaskId,
    model::ListView,
    query::QueryStatus,
    time::{Date, Weekday, days_between},
};
use todo_design::{layout::timeline as geometry, motion};

use super::{MenuKind, RoomKey, ShellView, component, desktop, elevation, kit, radius, space, ty};
use crate::{
    model::{
        self, Due, TaskRow,
        hints::Stop,
        labels,
        motion::Track,
        timeline::{self, Axis, Key},
    },
    theme::{Icon, TextStyled},
};

// In visual order: previous, today, next, four zooms, tray, task rail, completed.
pub(super) const CONTROL_COUNT: usize = 10;
const TRAY: usize = 7;
const RAIL: usize = 8;
const COMPLETED: usize = 9;
const ZOOM_LABELS: [(&str, &str); 4] = [
    ("Week", "1W"),
    ("2W", "2W"),
    ("Month", "M"),
    ("Quarter", "Q"),
];
const ZOOM_NAMES: [&str; 4] = ["Week", "Two weeks", "Month", "Quarter"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grip {
    Body,
    Start,
    Due,
    Tray,
}

#[derive(Clone, Copy)]
struct Drag {
    task: TaskId,
    before: Dates,
    grip: Grip,
    start: Point<Pixels>,
    current: Point<Pixels>,
    offset: f32,
    lifted: bool,
}

impl Drag {
    fn edit(self, axis: Axis, grid: Bounds<Pixels>) -> Option<Schedule> {
        if !self.lifted || !grid.contains(&self.current) {
            return None;
        }
        let dx = f32::from(self.current.x - self.start.x)
            + (axis.offset - self.offset) * axis.day_width();
        let by = axis.days_dragged(dx);
        Some(match self.grip {
            Grip::Body => Schedule::Move(by),
            Grip::Start => Schedule::StartEdge(by),
            Grip::Due => Schedule::DueEdge(by),
            Grip::Tray => Schedule::Place(axis.date_at(f32::from(self.current.x - grid.left()))?),
        })
    }
}

pub(super) struct TimelineUi {
    epoch: Option<Date>,
    offset: Track,
    pub cursor: Option<TaskId>,
    lanes: ScrollHandle,
    lane_bar: kit::Scrollbar,
    tray_scroll: kit::ScrollPane,
    grid: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    drag_frame: Duration,
    tray: Option<Track>,
    zoom_in: Track,
    pub focus: [FocusHandle; CONTROL_COUNT],
}

impl TimelineUi {
    pub fn new(cx: &Context<ShellView>) -> Self {
        Self {
            epoch: None,
            offset: Track::rest(0.),
            cursor: None,
            lanes: ScrollHandle::new(),
            lane_bar: kit::Scrollbar::default(),
            tray_scroll: kit::ScrollPane::default(),
            grid: Rc::default(),
            drag: None,
            drag_frame: Duration::ZERO,
            tray: None,
            zoom_in: Track::rest(1.),
            focus: std::array::from_fn(|_| cx.focus_handle()),
        }
    }

    pub fn reset_view(&mut self) {
        self.epoch = None;
        self.cursor = None;
        self.drag = None;
        self.tray = None;
        self.zoom_in = Track::rest(1.);
        self.lane_bar.reset_to_top(&self.lanes);
        self.tray_scroll.reset_to_top();
    }
}

impl ShellView {
    pub(super) fn on_timeline(&self) -> bool {
        self.view() == ListView::Timeline
    }

    fn timeline_rail(&self, window: &Window) -> f32 {
        if self.ui.timeline.rail {
            geometry::RAIL_WIDTH.min(self.room_width(window) / 2.)
        } else {
            geometry::RULER_HEIGHT
        }
    }

    fn timeline_axis(&self, window: &Window) -> Axis {
        let today = self.store.now().today();
        let zoom = self.ui.timeline.zoom;
        let epoch = self.timeline.epoch.unwrap_or(today);
        let axis = Axis {
            epoch,
            zoom,
            offset: self.timeline.epoch.map_or_else(
                || days_between(epoch, zoom.around_today(today)) as f32,
                |_| self.timeline.offset.at(self.frame),
            ),
            width: (self.room_width(window) - self.timeline_rail(window)).max(space::S1),
        };
        Axis {
            offset: axis.clamp(axis.offset),
            ..axis
        }
    }

    fn timeline_offset(
        &mut self,
        axis: Axis,
        offset: f32,
        duration: u32,
        curve: motion::CubicBezier,
    ) {
        let to = axis.clamp(offset);
        let now = self.now();
        let duration = if self.animate && !self.pace().reduced {
            duration
        } else {
            0
        };
        self.timeline.epoch = Some(axis.epoch);
        self.timeline.offset = Track::new(
            axis.offset,
            to,
            now,
            Duration::from_millis(duration.into()),
            curve,
        );
    }

    fn timeline_today(&mut self, window: &Window) {
        let axis = self.timeline_axis(window);
        let today = self.store.now().today();
        let offset = days_between(axis.epoch, axis.zoom.around_today(today)) as f32;
        self.timeline_offset(axis, offset, motion::PANEL, motion::STANDARD);
    }

    pub(super) fn timeline_counts(&self) -> (usize, usize, usize) {
        self.shell.room.timeline.as_ref().map_or((0, 0, 0), |data| {
            let completed = data
                .lanes
                .iter()
                .filter(|(lane, _)| lane.stage == QueryStatus::Done)
                .count();
            (
                data.lanes.len() - completed,
                data.unscheduled.len(),
                completed,
            )
        })
    }

    fn timeline_lanes(&self) -> impl Iterator<Item = &(Lane, TaskRow)> {
        self.shell
            .room
            .timeline
            .iter()
            .flat_map(|t| t.lanes.iter())
            .filter(|(lane, _)| self.ui.show_completed || lane.stage != QueryStatus::Done)
    }

    pub(super) fn timeline_ids(&self) -> Vec<TaskId> {
        self.timeline_lanes()
            .map(|(lane, _)| lane.task)
            .chain(
                self.shell
                    .room
                    .timeline
                    .iter()
                    .filter(|_| self.ui.timeline.tray)
                    .flat_map(|t| t.unscheduled.iter().map(|r| r.id)),
            )
            .collect()
    }

    fn timeline_selection(&self) -> Option<TaskId> {
        let ids = self.timeline_ids();
        self.timeline
            .cursor
            .filter(|id| ids.contains(id))
            .or_else(|| self.ui.task.filter(|id| ids.contains(id)))
    }

    pub(super) fn timeline_visible(&self, id: TaskId) -> bool {
        self.timeline_lanes().any(|(lane, _)| lane.task == id)
            || self
                .shell
                .room
                .timeline
                .as_ref()
                .is_some_and(|t| t.unscheduled.iter().any(|r| r.id == id))
    }

    fn timeline_lane_index(&self, id: TaskId) -> Option<usize> {
        self.timeline_lanes().position(|(l, _)| l.task == id)
    }

    fn reveal_timeline(&mut self, id: TaskId, window: &Window) {
        let lane = self
            .timeline_lanes()
            .enumerate()
            .find_map(|(i, (l, _))| (l.task == id).then_some((i, l.span)));
        if let Some((index, span)) = lane {
            let axis = self.timeline_axis(window);
            let offset = axis.reveal_offset(span);
            if offset != axis.offset {
                self.timeline_offset(axis, offset, 0, motion::STANDARD);
            }
            let bounds = self.timeline.lanes.bounds();
            let row_y = self.timeline.lanes.offset().y + px(geometry::LANE_ROW * index as f32);
            self.reveal_bounds(
                &self.timeline.lanes,
                Bounds::new(
                    point(bounds.left(), bounds.top() + row_y),
                    size(bounds.size.width, px(geometry::LANE_ROW)),
                ),
            );
        } else if let Some(i) = self
            .shell
            .room
            .timeline
            .as_ref()
            .and_then(|t| t.unscheduled.iter().position(|r| r.id == id))
        {
            let row = i / self.tray_columns(window);
            let scroll = &self.timeline.tray_scroll.handle;
            let bounds = scroll.bounds();
            let y = bounds.top()
                + scroll.offset().y
                + px((geometry::RULER_HEIGHT + space::S3) * row as f32);
            self.reveal_bounds(
                scroll,
                Bounds::new(
                    point(bounds.left(), y),
                    size(bounds.size.width, px(geometry::RULER_HEIGHT)),
                ),
            );
        }
    }

    pub(super) fn timeline_control_visible(&self, i: usize) -> bool {
        self.on_timeline()
            && match i {
                TRAY => self.timeline_counts().1 > 0,
                COMPLETED => {
                    self.timeline_counts().2 > 0
                        && !(self.width == model::layout::Width::Narrow
                            && self.motion.detail.look(self.frame).task > 0.)
                }
                _ => true,
            }
    }

    fn timeline_control(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
        if !self.timeline_control_visible(i) {
            return;
        }
        self.timeline.drag = None;
        let axis = self.timeline_axis(window);
        match i {
            0 | 2 => {
                let by = axis.zoom.days() as f32 * if i == 0 { -1. } else { 1. };
                let from = self
                    .timeline
                    .epoch
                    .map_or(axis.offset, |_| self.timeline.offset.target());
                self.timeline_offset(axis, from + by, motion::PANEL, motion::STANDARD);
            }
            1 => self.timeline_today(window),
            3..=6 => {
                let next = Zoom::ALL[i - 3];
                if next != axis.zoom {
                    self.timeline.epoch = Some(axis.epoch);
                    self.timeline.offset = Track::rest(axis.zoom_offset(next));
                    self.ui.timeline.zoom = next;
                    if self.animate {
                        let ms = if self.pace().reduced {
                            motion::REDUCED_FADE
                        } else {
                            motion::CONTENT_SWAP
                        };
                        // Keep the static room chrome at full contrast while the new grid fades in.
                        self.timeline.zoom_in = Track::new(
                            0.,
                            1.,
                            self.now(),
                            Duration::from_millis(ms.into()),
                            motion::STANDARD,
                        );
                    }
                    self.save_ui_preferences();
                }
            }
            TRAY => {
                let open = !self.ui.timeline.tray;
                let current = self.tray_height();
                self.ui.timeline.tray = open;
                let to = if open {
                    geometry::TRAY_EXPANDED
                } else {
                    geometry::TRAY_COLLAPSED
                };
                let duration = if self.animate && !self.pace().reduced {
                    Duration::from_millis(motion::PANEL.into())
                } else {
                    Duration::ZERO
                };
                self.timeline.tray = Some(Track::new(
                    current,
                    to,
                    self.now(),
                    duration,
                    if open { motion::ENTER } else { motion::EXIT },
                ));
                self.save_ui_preferences();
            }
            RAIL => {
                self.ui.timeline.rail = !self.ui.timeline.rail;
                self.save_ui_preferences();
            }
            COMPLETED => self.ui.show_completed = !self.ui.show_completed,
            _ => return,
        }
        cx.notify();
    }

    fn timeline_key_down(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.list_focus.is_focused(window) {
            return false;
        }
        let Some((key, act)) = timeline::key(e) else {
            return self.workflow_key(e, window, cx);
        };
        if !act {
            return true;
        }
        let selected = self.timeline_selection();
        let task = selected.or_else(|| self.timeline_ids().first().copied());
        match key {
            Key::Lane(by) => {
                let ids = self.timeline_ids();
                let next = match selected {
                    Some(id) => ids
                        .iter()
                        .position(|&t| t == id)
                        .and_then(|index| index.checked_add_signed(by))
                        .and_then(|i| ids.get(i).copied()),
                    None => task,
                };
                if let Some(id) = next {
                    if self.ui.task.is_some() {
                        self.preview_task(Some(id), cx);
                    }
                    self.timeline.cursor = Some(id);
                    self.reveal_timeline(id, window);
                }
            }
            Key::Shift(by) | Key::Duration(by) => {
                if let Some(id) = task {
                    let state = self.store.view();
                    if let Some(t) = state.tasks.get(&id) {
                        let edit = if matches!(key, Key::Shift(_)) {
                            Schedule::Move(by)
                        } else {
                            Schedule::Duration(by)
                        };
                        self.apply_schedule(id, Dates::of(t), edit, window, cx);
                    }
                }
            }
            Key::Today => self.timeline_today(window),
            Key::Calendar(starts) => {
                if let Some(id) = task {
                    self.timeline_calendar(id, starts, None, window, cx);
                }
            }
            Key::Open => {
                if let Some(id) = task {
                    self.select_task(Some(id), cx);
                }
            }
            Key::Complete => {
                if let Some(id) = task {
                    self.toggle_complete(id, cx);
                }
            }
            Key::Escape => {
                if self.timeline.drag.take().is_none() {
                    self.select_task(None, cx);
                    self.timeline.cursor = None;
                }
            }
        }
        cx.notify();
        true
    }

    fn timeline_calendar(
        &mut self,
        id: TaskId,
        starts: bool,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        let Some(task) = state.tasks.get(&id) else {
            return;
        };
        if !todo_client::timeline::includes(&state, model::shared_key(self.ui.room), task) {
            return;
        }
        let dates = Dates::of(task);
        let calendar = super::quick_pickers::Calendar::new(
            Some(id),
            self.store.now().today(),
            if starts { dates.start } else { dates.due },
        );
        let calendar = if starts {
            calendar.for_start()
        } else {
            calendar
        };
        let anchor = at.unwrap_or_else(|| {
            let grid = self.timeline.grid.get();
            let index = self.timeline_lane_index(id);
            let y = index
                .map_or(grid.bottom(), |i| {
                    grid.top()
                        + self.timeline.lanes.offset().y
                        + px((i + 1) as f32 * geometry::LANE_ROW)
                })
                .clamp(grid.top(), grid.bottom());
            let axis = self.timeline_axis(window);
            let x = dates
                .start
                .or(dates.due)
                .map_or(0., |day| axis.x(day))
                .clamp(0., axis.width);
            point(grid.left() + px(x), y)
        });
        self.timeline.cursor = Some(id);
        self.open_menu(
            MenuKind::Date(calendar.for_timeline(dates)),
            Some(anchor),
            Some(Stop::List),
            window,
            cx,
        );
    }

    pub(super) fn apply_schedule(
        &mut self,
        id: TaskId,
        before: Dates,
        edit: Schedule,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.on_timeline() {
            return false;
        }
        if !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return false;
        }
        let state = self.store.view();
        let room = model::shared_key(self.ui.room);
        let Some(command) = workflow::schedule(&state, room, id, before, edit) else {
            return false;
        };
        let expected = command.clone();
        let saved = self.submit_if_kind(
            command,
            id.into(),
            super::undo::EditKind::Schedule,
            |live| workflow::schedule(live, room, id, before, edit).as_ref() == Some(&expected),
        ) == Ok(true);
        self.refresh(cx);
        if saved {
            self.timeline.cursor = Some(id);
            self.reveal_timeline(id, window);
        }
        saved
    }

    fn start_timeline_drag(
        &mut self,
        id: TaskId,
        grip: Grip,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        let Some(task) = state.tasks.get(&id).filter(|t| {
            todo_client::timeline::includes(&state, model::shared_key(self.ui.room), t)
        }) else {
            return;
        };
        let axis = self.timeline_axis(window);
        self.timeline_offset(axis, axis.offset, 0, motion::STANDARD);
        self.timeline.drag = Some(Drag {
            task: id,
            before: Dates::of(task),
            grip,
            start: at,
            current: at,
            offset: axis.offset,
            lifted: false,
        });
        self.timeline.cursor = Some(id);
        self.timeline.drag_frame = self.now();
        window.focus(&self.list_focus, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn drive_timeline(&mut self, window: &mut Window) {
        if !self.on_timeline() {
            return;
        }
        if self.timeline.offset.moving(self.frame)
            || self.timeline.zoom_in.moving(self.frame)
            || self
                .timeline
                .tray
                .is_some_and(|track| track.moving(self.frame))
        {
            window.request_animation_frame();
        }
        let Some(drag) = self.timeline.drag.filter(|g| g.lifted) else {
            return;
        };
        let state = self.store.view();
        if !window.is_window_active()
            || !self.timeline_visible(drag.task)
            || state.tasks.get(&drag.task).is_none_or(|task| {
                Dates::of(task) != drag.before
                    || !todo_client::timeline::includes(
                        &state,
                        model::shared_key(self.ui.room),
                        task,
                    )
            })
        {
            self.timeline.drag = None;
            return;
        }
        let grid = self.timeline.grid.get();
        let now = self.now();
        let last = std::mem::replace(&mut self.timeline.drag_frame, now);
        if !grid.contains(&drag.current) {
            return;
        }
        let edge = px(space::S6);
        let speed = if drag.current.x < grid.left() + edge {
            -(grid.left() + edge - drag.current.x) / edge
        } else if drag.current.x > grid.right() - edge {
            (drag.current.x - (grid.right() - edge)) / edge
        } else {
            0.
        };
        if speed != 0. {
            let axis = self.timeline_axis(window);
            let elapsed = now
                .saturating_sub(last)
                .min(Duration::from_millis(motion::HOVER.into()));
            let step = speed * elapsed.as_secs_f32()
                / Duration::from_millis(motion::PANEL.into()).as_secs_f32();
            self.timeline_offset(axis, axis.offset + step, 0, motion::STANDARD);
            window.request_animation_frame();
        }
    }

    pub(super) fn timeline_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                            let Some(drag) = &mut this.timeline.drag else {
                                return;
                            };
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.timeline.drag = None;
                                cx.notify();
                                return;
                            }
                            drag.current = e.position;
                            let delta = (drag.current - drag.start).map(f32::from);
                            drag.lifted |= delta.x.abs().max(delta.y.abs()) >= space::S1;
                            if drag.lifted {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        })
                        .ok();
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        let Some(mut drag) = this.timeline.drag.take().filter(|g| g.lifted) else {
                            return;
                        };
                        drag.current = e.position;
                        if let Some(edit) =
                            drag.edit(this.timeline_axis(window), this.timeline.grid.get())
                        {
                            this.apply_schedule(drag.task, drag.before, edit, window, cx);
                        }
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

    fn timeline_button(
        &self,
        i: usize,
        name: &'static str,
        marked: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let focus = &self.timeline.focus[i];
        div()
            .id(("timeline-control", i))
            .track_focus(focus)
            .role(Role::Button)
            .aria_label(name)
            .tooltip(kit::tooltip(p, name))
            .cursor_pointer()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(space::S1))
            .h(px(component::BUTTON_SMALL))
            .px(px(space::S2))
            .rounded(px(radius::MD))
            .style(ty::BUTTON_SMALL.with_weight(if marked { 600 } else { 500 }))
            .text_color(if marked { p.accent_strong } else { p.ink })
            .bg(if marked { p.wash } else { p.raised })
            .border(px(elevation::HAIRLINE))
            .border_color(if focus.is_focused(window) || marked {
                p.accent_strong
            } else {
                p.line_strong
            })
            .hover(|d| d.bg(p.hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.timeline_control(i, window, cx);
            }))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                if !e.keystroke.modifiers.modified()
                    && matches!(e.keystroke.key.as_str(), "enter" | "space")
                {
                    cx.stop_propagation();
                    if !e.is_held {
                        this.timeline_control(i, window, cx);
                    }
                }
            }))
    }

    fn timeline_toolbar(&self, axis: Axis, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let compact = self.room_width(window) < desktop::LIST_MIN;
        let (first, last) = axis.range();
        let range = format!(
            "{} · {}–{}",
            self.store.now().dates().range(first, last),
            week(first),
            week(last)
        );
        let unscheduled = self.timeline_counts().1;
        div()
            .h(px(geometry::TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(if compact {
                space::S2
            } else {
                desktop::LIST_PAD_X
            }))
            .border_b_1()
            .border_color(p.line)
            .child(
                self.timeline_button(0, "Previous dates", false, window, cx)
                    .child(kit::icon(Icon::ChevronLeft, component::ICON_SMALL, p.ink_2)),
            )
            .child(
                self.timeline_button(1, "Today", false, window, cx)
                    .child("Today"),
            )
            .child(
                self.timeline_button(2, "Next dates", false, window, cx)
                    .child(kit::icon(
                        Icon::ChevronRight,
                        component::ICON_SMALL,
                        p.ink_2,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .style(ty::COUNT)
                    .text_color(p.ink_2)
                    .child(range),
            )
            .child(
                div()
                    .id("timeline-zoom")
                    .role(Role::TabList)
                    .aria_label("Timeline zoom")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(elevation::HAIRLINE))
                    .p(px(elevation::HAIRLINE))
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line)
                    .bg(p.canvas)
                    .children(Zoom::ALL.into_iter().enumerate().map(|(i, zoom)| {
                        let active = axis.zoom == zoom;
                        self.timeline_button(i + 3, ZOOM_NAMES[i], active, window, cx)
                            .role(Role::Tab)
                            .aria_selected(active)
                            .h(px(component::VIEW_SWITCHER - space::S1))
                            .bg(if active { p.raised } else { p.canvas })
                            .border_color(if self.timeline.focus[i + 3].is_focused(window) {
                                p.accent_strong
                            } else if active {
                                p.line_strong
                            } else {
                                gpui::transparent_black()
                            })
                            .child(if compact {
                                ZOOM_LABELS[i].1
                            } else {
                                ZOOM_LABELS[i].0
                            })
                    })),
            )
            .when(unscheduled > 0, |bar| {
                bar.child(
                    self.timeline_button(
                        TRAY,
                        "Unscheduled tasks",
                        self.ui.timeline.tray,
                        window,
                        cx,
                    )
                    .aria_expanded(self.ui.timeline.tray)
                    .child(kit::icon(Icon::Calendar, component::ICON_SMALL, p.ink_2))
                    .child(if compact {
                        unscheduled.to_string()
                    } else {
                        format!("Unscheduled · {unscheduled}")
                    }),
                )
            })
    }

    pub(super) fn timeline_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let axis = self.timeline_axis(window);
        let today = self.store.now().today();
        let p = self.palette;
        let rail = self.timeline_rail(window);
        let shown = self.timeline_lanes().count();
        let graph = div()
            .id("timeline-grid")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden();
        let grid = self.timeline.grid.clone();
        let background = canvas(
            move |bounds, _, _| {
                grid.set(bounds);
            },
            move |bounds, (), window, _| {
                let paint = |window: &mut Window, x: f32, width: f32, color| {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.left() + px(x), bounds.top()),
                            size(px(width), bounds.size.height),
                        ),
                        color,
                    ));
                };
                for (day, x, width) in columns(axis) {
                    if axis.zoom != Zoom::Quarter && is_weekend(day) {
                        paint(window, x, width, p.canvas);
                    }
                    paint(window, x, elevation::HAIRLINE, p.line);
                }
                let x = today_x(axis, today);
                if (0. ..axis.width).contains(&x) {
                    paint(
                        window,
                        x - geometry::TODAY_LINE / 2.,
                        geometry::TODAY_LINE,
                        p.accent_strong,
                    );
                }
            },
        )
        .absolute()
        .left(px(rail))
        .right_0()
        .top_0()
        .bottom_0();
        let cursor = self.timeline_selection();
        let view = div()
            .id("timeline")
            .role(Role::Group)
            .aria_label("Task timeline")
            .track_focus(&self.list_focus)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .opacity(self.board.opacity(self.frame))
            .on_key_down(cx.listener(|this, e, window, cx| {
                if this.timeline_key_down(e, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.timeline_toolbar(axis, window, cx));
        let ruler = self.timeline_ruler(axis, rail, today, shown, window, cx);
        let rows = div()
            .id("timeline-lanes")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.timeline.lanes)
            .children(self.timeline_lanes().enumerate().map(|(i, (lane, row))| {
                let selected = self.ui.task == Some(row.id)
                    || cursor == Some(row.id) && self.list_focus.is_focused(window);
                self.timeline_lane(i, *lane, row, axis, rail, selected, window, cx)
            }));
        div().flex_1().min_h_0().child(
            view.size_full()
                .child(
                    div()
                        .id("timeline-pan")
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .flex_col()
                        .opacity(self.timeline.zoom_in.at(self.frame))
                        .on_scroll(cx.listener(|this, e: &gpui::ScrollEvent, window, cx| {
                            let d = e.delta.pixel_delta(px(geometry::LANE_ROW));
                            if this.timeline.drag.is_some() {
                                cx.stop_propagation();
                                return;
                            }
                            let dx = if e.modifiers.shift && d.x == px(0.) {
                                d.y
                            } else {
                                d.x
                            };
                            if dx != px(0.) {
                                let axis = this.timeline_axis(window);
                                this.timeline_offset(
                                    axis,
                                    axis.offset - f32::from(dx) / axis.day_width(),
                                    0,
                                    motion::STANDARD,
                                );
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child(ruler)
                        .child(
                            graph.child(background).child(rows).child(
                                self.timeline.lane_bar.overlay(
                                    &self.timeline.lanes,
                                    &p,
                                    self.animate,
                                ),
                            ),
                        ),
                )
                .children(self.timeline_tray(window, cx)),
        )
    }

    fn timeline_ruler(
        &self,
        axis: Axis,
        rail: f32,
        today: Date,
        count: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let open = self.ui.timeline.rail;
        let x = today_x(axis, today);
        div()
            .h(px(geometry::RULER_HEIGHT))
            .flex_none()
            .flex()
            .bg(p.canvas)
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .w(px(rail))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(if open {
                        (rail / 8.).min(desktop::LIST_PAD_X)
                    } else {
                        space::S1
                    }))
                    .pr(px(space::S1))
                    .child(
                        self.timeline_button(
                            RAIL,
                            if open {
                                "Collapse task rail"
                            } else {
                                "Expand task rail"
                            },
                            false,
                            window,
                            cx,
                        )
                        .aria_expanded(open)
                        .bg(p.canvas)
                        .border_color(if self.timeline.focus[RAIL].is_focused(window) {
                            p.accent_strong
                        } else {
                            p.canvas
                        })
                        .when(open, |d| d.child("Task"))
                        .child(kit::icon(
                            if open {
                                Icon::ChevronLeft
                            } else {
                                Icon::ChevronRight
                            },
                            component::ICON_SMALL,
                            p.ink_3,
                        )),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_right()
                                .pr(px(space::S2))
                                .style(ty::CAPS_LABEL)
                                .text_color(p.ink_3)
                                .child(format!("{count} shown").to_uppercase()),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .children(columns(axis).into_iter().map(|(day, x, width)| {
                        let quarter = axis.zoom == Zoom::Quarter;
                        let is_today =
                            today >= day && days_between(day, today) < axis.zoom.column_days();
                        let weekday = labels::weekday(day);
                        let top = if quarter {
                            week(day)
                        } else {
                            weekday[..if axis.zoom == Zoom::Month { 1 } else { 3 }].to_uppercase()
                        };
                        let bottom = if quarter || day.day() == 1 && axis.zoom != Zoom::Month {
                            self.store.now().dates().date(day)
                        } else {
                            day.day().to_string()
                        };
                        let iso =
                            !quarter && (Weekday::of(day) == Weekday::Mon || day == axis.range().0);
                        div()
                            .absolute()
                            .left(px(x))
                            .w(px(width))
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_end()
                            .pb(px(space::S1 + elevation::HAIRLINE))
                            .border_l_1()
                            .border_color(p.line)
                            .when(is_today, |d| d.bg(p.wash))
                            .text_color(if is_today { p.accent_strong } else { p.ink_2 })
                            .child(div().style(ty::AGENT_LETTER).child(top))
                            .child(div().style(ty::COUNT.with_weight(600)).child(bottom))
                            .when(iso, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(elevation::HAIRLINE))
                                        .left(px(space::S1))
                                        .style(ty::AGENT_LETTER)
                                        .text_color(p.ink_3)
                                        .child(week(day)),
                                )
                            })
                    }))
                    .when((0. ..axis.width).contains(&x), |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left(px(x - geometry::TODAY_LINE / 2.))
                                .w(px(geometry::TODAY_LINE))
                                .h(px(space::S1))
                                .bg(p.accent_strong),
                        )
                    }),
            )
    }

    #[expect(clippy::too_many_arguments, reason = "one lane and its positioned bar")]
    fn timeline_lane(
        &self,
        i: usize,
        lane: Lane,
        row: &TaskRow,
        axis: Axis,
        rail: f32,
        selected: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let cross = !matches!(self.ui.room, RoomKey::List(_));
        let list = crate::theme::list_colors(self.theme, row.list.hue);
        let open = self.ui.timeline.rail;
        let id = row.id;
        let stage = if !self.shell.room.features.workflow && lane.stage != QueryStatus::Done {
            QueryStatus::Todo
        } else {
            lane.stage
        };
        let (stage_name, stage_icon) = model::workflow::status(stage);
        let range = self
            .store
            .now()
            .dates()
            .range(lane.span.first, lane.span.last);
        let label = format!("{} · {stage_name} · {range}", row.title);
        let transition = self.motion.row(id, self.frame);
        let look = transition.unwrap_or_else(|| super::RowLook::rest(row.done));
        div()
            .h(px(geometry::LANE_ROW * look.height))
            .opacity(look.opacity)
            .overflow_hidden()
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .id(("timeline-task", i))
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .when(selected && self.list_focus.is_focused(window), |d| {
                        d.aria_active_descendant()
                    })
                    .tooltip(kit::tooltip(p, label))
                    .cursor_pointer()
                    .relative()
                    .flex()
                    .items_center()
                    .flex_none()
                    .w(px(rail))
                    .h_full()
                    .pl(px(if open {
                        (rail / 7.).min(desktop::LIST_PAD_X)
                    } else {
                        space::S3
                    }))
                    .pr(px(space::S3))
                    .gap(px(space::S2))
                    .bg(if selected { p.wash } else { p.pane })
                    .border_r_1()
                    .border_color(p.line)
                    .hover(|d| d.bg(p.hover))
                    .when(selected, |d| {
                        d.child(kit::leading_rail(p.accent_strong, elevation::SELECTION_BAR))
                    })
                    .child(if transition.is_some() {
                        kit::checkbox_at(
                            &p,
                            matches!(row.due, Due::Overdue(_)),
                            selected,
                            component::ICON_SMALL,
                            (look.fill, look.scale, look.tick),
                        )
                        .into_any_element()
                    } else if cross {
                        kit::glyph_at(
                            crate::model::lists::list_glyph(&row.list.name),
                            list.glyph,
                            component::GLYPH_SMALL,
                        )
                        .into_any_element()
                    } else {
                        kit::icon(
                            stage_icon,
                            component::ICON_SMALL,
                            if matches!(row.due, Due::Overdue(_)) {
                                p.danger
                            } else {
                                super::workflow::status_ink(&p, stage, selected)
                            },
                        )
                        .into_any_element()
                    })
                    .when(open, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .style(ty::BUTTON.with_weight(500))
                                .when(look.strike > 0., |d| d.line_through())
                                .text_color(p.ink)
                                .child(row.title.clone()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.timeline.cursor = Some(id);
                        this.reveal_timeline(id, window);
                        this.select_task(Some(id), cx);
                        window.focus(&this.list_focus, cx);
                    })),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .children(self.timeline_bar(lane, row, axis, selected, window, cx)),
            )
    }

    fn timeline_bar(
        &self,
        lane: Lane,
        row: &TaskRow,
        axis: Axis,
        selected: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let p = self.palette;
        let id = row.id;
        let drag = self.timeline.drag.filter(|g| g.task == id && g.lifted);
        let preview = drag.and_then(|d| {
            d.edit(axis, self.timeline.grid.get())
                .and_then(|edit| edit.preview(d.before))
        });
        let span = preview
            .and_then(|d| Span::from_dates(d.start, d.due))
            .unwrap_or(lane.span);
        let natural_left = axis.x(span.first);
        let natural_right = axis.x(span.last) + axis.day_width();
        if natural_right <= 0. || natural_left >= axis.width {
            return None;
        }
        let pad = bar_inset(axis);
        let left = (natural_left + pad).max(0.);
        let right = (natural_right - pad)
            .min(axis.width)
            .max(left + elevation::HAIRLINE);
        let width = right - left;
        let title_style = ty::BUTTON.with_weight(500);
        let title_width = kit::text_width(window, title_style, &row.title);
        let metadata_width = if row.agent.is_some() {
            component::AGENT_MARK + space::S2
        } else if let Some((done, total)) = row.steps {
            kit::text_width(window, ty::COUNT, &format!("{done}/{total}")) + space::S2
        } else {
            0.
        };
        let inline = width >= geometry::INLINE_LABEL_MIN
            && span.kind != SpanKind::Milestone
            && (selected
                || width >= title_width + metadata_width + 2. * (geometry::RESIZE_HIT + space::S1)
                || (axis.width - right).max(left) <= width);
        let list = crate::theme::list_colors(self.theme, row.list.hue);
        let cross = !matches!(self.ui.room, RoomKey::List(_));
        let overdue = matches!(row.due, Due::Overdue(_));
        let progress = lane.stage == QueryStatus::InProgress && self.shell.room.features.workflow;
        let backlog = lane.stage == QueryStatus::Backlog && self.shell.room.features.workflow;
        let (fill, edge, ink, cap) = if cross {
            (list.tint, list.swatch, list.ink, Some(list.swatch))
        } else if overdue {
            (p.danger_bg, p.danger, p.danger_ink, None)
        } else if progress {
            (p.wash, p.accent, p.ink, Some(p.accent_strong))
        } else if selected {
            (p.wash, p.accent_strong, p.ink, None)
        } else if backlog {
            (p.canvas, p.line_strong, p.ink_2, None)
        } else {
            (p.raised, p.line_strong, p.ink, None)
        };
        let bar_top = (geometry::LANE_ROW - geometry::BAR_HEIGHT) / 2.;
        let date_readout = self.timeline_date_readout(span);
        let tooltip = if cross {
            format!("{} · {} · {date_readout}", row.title, row.list.name)
        } else {
            format!("{} · {date_readout}", row.title)
        };
        let bar = div()
            .id(SharedString::from(format!("timeline-bar-{id}")))
            .role(Role::Button)
            .aria_label(tooltip.clone())
            .aria_selected(selected)
            .tooltip(kit::tooltip(p, tooltip))
            .absolute()
            .left(px(left))
            .top(px(bar_top))
            .w(px(width))
            .h(px(geometry::BAR_HEIGHT))
            .rounded(px(radius::MD))
            .bg(fill)
            .border(px(elevation::HAIRLINE))
            .border_color(edge)
            .when(backlog && !overdue && !cross, |d| d.border_dashed())
            .flex()
            .items_center()
            .justify_center()
            .gap(px(space::S2))
            .overflow_hidden()
            .text_color(ink)
            .cursor_grab()
            .hover(|d| {
                d.border_color(if overdue && !cross {
                    p.danger_ink
                } else if cross {
                    list.ink
                } else {
                    p.accent_strong
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.start_timeline_drag(id, Grip::Body, e.position, window, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_task(Some(id), cx);
                window.focus(&this.list_focus, cx);
            }))
            .children(cap.map(|color| kit::leading_rail(color, elevation::SELECTION_BAR)))
            .when(inline, |d| {
                d.pl(px(geometry::RESIZE_HIT + space::S1))
                    .pr(px(geometry::RESIZE_HIT + space::S1))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .style(ty::BUTTON.with_weight(500))
                            .child(row.title.clone()),
                    )
            })
            .when(span.kind == SpanKind::Milestone, |d| {
                d.child(kit::icon(
                    Icon::Flag,
                    component::ICON_SMALL,
                    if overdue { p.danger_ink } else { ink },
                ))
            })
            .when(span.kind != SpanKind::Milestone, |d| {
                if let Some(agent) = row.agent.filter(|_| inline) {
                    d.child(kit::agent_mark(self.theme, agent, component::AGENT_MARK))
                } else if let Some((done, total)) = row.steps.filter(|_| width >= space::S8) {
                    d.child(
                        div()
                            .flex_none()
                            .style(ty::COUNT)
                            .text_color(ink)
                            .child(format!("{done}/{total}")),
                    )
                } else {
                    d
                }
            })
            .when(
                span.kind == SpanKind::OpenEnded && natural_right <= axis.width,
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(geometry::RESIZE_HIT))
                            .bg(fill)
                            .border_r_1()
                            .border_dashed()
                            .border_color(edge),
                    )
                },
            )
            .when(
                natural_left >= 0. && span.kind != SpanKind::OpenEnded,
                |d| d.child(self.timeline_handle(id, Grip::Start, width, selected, ink, cx)),
            )
            .when(
                natural_right <= axis.width && span.kind != SpanKind::Milestone,
                |d| d.child(self.timeline_handle(id, Grip::Due, width, selected, ink, cx)),
            );
        let external = (!inline).then(|| {
            let gap = space::S2;
            let detail = if let Due::Overdue(s) = &row.due {
                s.to_string()
            } else {
                self.store.now().dates().range(span.first, span.last)
            };
            let padded = |width: f32| width.ceil() + 2. * space::S1 + elevation::HAIRLINE;
            let full = format!("{} · {detail}", row.title);
            let full_width = padded(kit::text_width(window, title_style, &full));
            let title_width = padded(title_width);
            let after = (axis.width - right - gap).max(0.);
            let before = (left - gap).max(0.);
            let on_right = after >= title_width || after >= before;
            let available = if on_right { after } else { before };
            // On a crowded scale, keep the whole title before giving its date a place.
            let (label, wanted) = if available >= full_width {
                (SharedString::from(full), full_width)
            } else {
                (row.title.clone(), title_width)
            };
            let width = wanted.min(available);
            let x = if on_right {
                right + gap
            } else {
                before - width
            };
            div()
                .id(SharedString::from(format!("timeline-label-{id}")))
                .absolute()
                .left(px(x))
                .top(px(
                    bar_top + (geometry::BAR_HEIGHT - title_style.line_height) / 2.
                ))
                .w(px(width))
                .px(px(space::S1))
                .rounded(px(radius::SM))
                .truncate()
                .style(title_style)
                .bg(p.pane)
                .text_color(if cross { p.ink } else { ink })
                .cursor_pointer()
                .tooltip(kit::tooltip(p, format!("{} · {date_readout}", row.title)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_task(Some(id), cx);
                    window.focus(&this.list_focus, cx);
                }))
        });
        Some(div().size_full().child(bar).children(external))
    }

    fn timeline_handle(
        &self,
        id: TaskId,
        grip: Grip,
        width: f32,
        selected: bool,
        ink: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let start = grip == Grip::Start;
        let name = if start {
            "Drag to change start date"
        } else {
            "Drag to change due date"
        };
        div()
            .id(if start { "resize-start" } else { "resize-due" })
            .absolute()
            .top_0()
            .bottom_0()
            .when(start, |d| d.left_0())
            .when(!start, |d| d.right_0())
            .w(px(geometry::RESIZE_HIT.min(width / 3.)))
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .aria_label(name)
            .tooltip(kit::tooltip(self.palette, name))
            .hover(|d| d.bg(self.palette.hover))
            .when(selected, |d| {
                d.child(
                    div()
                        .w(px(elevation::HAIRLINE))
                        .h(px(space::S3))
                        .rounded_full()
                        .bg(ink),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.start_timeline_drag(id, grip, e.position, window, cx)
                }),
            )
    }

    fn timeline_date_readout(&self, span: Span) -> String {
        let dates = self.store.now().dates();
        let first = dates.date(span.first);
        match span.kind {
            SpanKind::Milestone => format!("Due {first}"),
            SpanKind::OpenEnded => format!("Starts {first}"),
            SpanKind::Range => format!(
                "{} · {}d",
                dates.range(span.first, span.last),
                days_between(span.first, span.last) + 1
            ),
        }
    }

    pub(super) fn timeline_preview(&self, window: &Window) -> Option<Div> {
        let drag = self.timeline.drag?;
        let axis = self.timeline_axis(window);
        let bounds = self.timeline.grid.get();
        let next = drag
            .edit(axis, bounds)?
            .preview(drag.before)
            .unwrap_or(drag.before);
        let span = Span::from_dates(next.start, next.due)?;
        let p = self.palette;
        let text = self.timeline_date_readout(span);
        let width = kit::text_width(window, ty::DUE, &text) + 2. * space::S2;
        let x = (drag.current.x + px(space::S3))
            .min(bounds.right() - px(width))
            .max(bounds.left());
        let y = (drag.current.y - px(geometry::BAR_HEIGHT + space::S2))
            .max(bounds.top() - px(geometry::RULER_HEIGHT));
        Some(
            div()
                .absolute()
                .left_0()
                .top_0()
                .size_0()
                .when(drag.grip == Grip::Tray, |d| {
                    let row = ((drag.current.y - bounds.top() - self.timeline.lanes.offset().y)
                        / px(geometry::LANE_ROW))
                    .floor();
                    let top = bounds.top()
                        + self.timeline.lanes.offset().y
                        + px(row * geometry::LANE_ROW);
                    let pad = bar_inset(axis);
                    let left = (bounds.left() + px(axis.x(span.first) + pad)).max(bounds.left());
                    let right = (bounds.left() + px(axis.x(span.first) + axis.day_width() - pad))
                        .min(bounds.right());
                    d.child(
                        div()
                            .absolute()
                            .left(left)
                            .top(top + px((geometry::LANE_ROW - geometry::BAR_HEIGHT) / 2.))
                            .w(right - left)
                            .h(px(geometry::BAR_HEIGHT))
                            .rounded(px(radius::MD))
                            .border(px(elevation::HAIRLINE))
                            .border_color(p.accent_strong)
                            .bg(p.wash)
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(kit::icon(
                                Icon::Flag,
                                component::ICON_SMALL,
                                p.accent_strong,
                            )),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(px(width))
                        .px(px(space::S2))
                        .py(px(space::S1))
                        .rounded(px(radius::SM))
                        .bg(p.toast)
                        .text_color(p.toast_ink)
                        .style(ty::DUE)
                        .child(text),
                ),
        )
    }

    fn tray_height(&self) -> f32 {
        self.timeline.tray.map_or(
            if self.ui.timeline.tray {
                geometry::TRAY_EXPANDED
            } else {
                geometry::TRAY_COLLAPSED
            },
            |track| track.at(self.frame),
        )
    }

    fn tray_columns(&self, window: &Window) -> usize {
        if self.room_width(window) - 2. * desktop::LIST_PAD_X
            >= desktop::LIST_MIN + geometry::RAIL_WIDTH
        {
            2
        } else {
            1
        }
    }

    fn timeline_tray(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let tasks = &self.shell.room.timeline.as_ref()?.unscheduled;
        if tasks.is_empty() {
            return None;
        }
        let p = self.palette;
        let open = self.ui.timeline.tray;
        let height = self.tray_height();
        let columns = self.tray_columns(window);
        let cursor = self
            .timeline_selection()
            .filter(|_| self.list_focus.is_focused(window));
        let heading = div()
            .id("timeline-tray-toggle")
            .role(Role::Button)
            .aria_expanded(open)
            .aria_label(format!("Unscheduled, {} tasks", tasks.len()))
            .cursor_pointer()
            .h(px(geometry::TRAY_COLLAPSED))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(space::S2))
            .px(px(desktop::LIST_PAD_X))
            .style(ty::CAPS_LABEL)
            .text_color(p.ink_3)
            .hover(|d| d.bg(p.hover))
            .child(div().min_w_0().truncate().child(format!(
                "UNSCHEDULED · {}{}",
                tasks.len(),
                if open {
                    " — DRAG ONTO A DATE TO SCHEDULE"
                } else {
                    ""
                }
            )))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .text_color(p.accent_strong)
                    .child(if open { "HIDE TRAY" } else { "SHOW TRAY" })
                    .child(kit::icon(
                        if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronUp
                        },
                        component::ICON_SMALL,
                        p.accent_strong,
                    )),
            )
            .on_click(cx.listener(|this, _, window, cx| this.timeline_control(TRAY, window, cx)));
        let cards = (height > geometry::TRAY_COLLAPSED || open).then(|| {
            self.timeline
                .tray_scroll
                .viewport("unscheduled-cards")
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S3))
                        .px(px(desktop::LIST_PAD_X))
                        .pb(px(space::S3))
                        .children(tasks.chunks(columns).map(|rows| {
                            div()
                                .flex()
                                .gap(px(space::S3))
                                .children(
                                    rows.iter()
                                        .map(|row| self.tray_card(row, cursor == Some(row.id), cx)),
                                )
                                .when(rows.len() < columns, |d| d.child(div().flex_1()))
                        })),
                )
        });
        Some(
            div()
                .h(px(height))
                .flex_none()
                .flex()
                .flex_col()
                .overflow_hidden()
                .bg(p.canvas)
                .border_t_1()
                .border_color(p.line)
                .child(heading)
                .children(cards.map(|cards| {
                    div().relative().flex_1().min_h_0().child(cards).child(
                        self.timeline.tray_scroll.bar.overlay(
                            &self.timeline.tray_scroll.handle,
                            &p,
                            self.animate,
                        ),
                    )
                })),
        )
    }

    fn tray_card(&self, row: &TaskRow, selected: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.palette;
        let id = row.id;
        let schedule = div()
            .id("schedule")
            .role(Role::Button)
            .aria_label(format!("Schedule {}", row.title))
            .cursor_pointer()
            .flex_none()
            .h(px(component::BUTTON_SMALL))
            .flex()
            .items_center()
            .px(px(space::S2))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .border_color(p.line_strong)
            .hover(|d| d.bg(p.hover))
            .style(ty::BUTTON_SMALL)
            .child("Schedule")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                this.timeline_calendar(id, false, e.mouse_position(), window, cx);
            }));
        div()
            .id(SharedString::from(format!("unscheduled-task-{id}")))
            .role(Role::Group)
            .aria_label(row.title.clone())
            .when(selected, |d| d.aria_active_descendant())
            .min_w_0()
            .flex_1()
            .h(px(geometry::RULER_HEIGHT))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S3))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .cursor_grab()
            .border_color(if selected { p.accent_strong } else { p.line })
            .bg(if selected { p.wash } else { p.raised })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.start_timeline_drag(id, Grip::Tray, e.position, window, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select_task(Some(id), cx)))
            .child(kit::icon(
                Icon::GripVertical,
                component::ICON_SMALL,
                p.ink_3,
            ))
            .children(row.status.map(|stage| {
                kit::icon(
                    model::workflow::status(stage).1,
                    component::ICON_SMALL,
                    super::workflow::status_ink(&p, stage, selected),
                )
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .style(ty::BUTTON.with_weight(500))
                    .child(row.title.clone()),
            )
            .children(row.tags.first().map(|tag| {
                div()
                    .max_w(px(desktop::CHIP_MAX / 2.))
                    .truncate()
                    .style(ty::TAG)
                    .text_color(crate::theme::list_colors(self.theme, tag.hue).ink)
                    .child(tag.label.clone())
            }))
            .children(
                row.agent
                    .map(|agent| kit::agent_mark(self.theme, agent, component::AGENT_MARK)),
            )
            .child(schedule)
    }

    pub(super) fn timeline_footer(
        &self,
        enabled: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.on_timeline() {
            return None;
        }
        let p = self.palette;
        let (_, _, completed) = self.timeline_counts();
        Some(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(space::S2))
                .when(completed > 0 && enabled, |d| {
                    d.child(
                        self.timeline_button(
                            COMPLETED,
                            "Show or hide completed timeline tasks",
                            false,
                            window,
                            cx,
                        )
                        .aria_expanded(self.ui.show_completed)
                        .style(ty::CAPS_LABEL)
                        .bg(p.pane)
                        .border_color(if self.timeline.focus[COMPLETED].is_focused(window) {
                            p.accent_strong
                        } else {
                            p.pane
                        })
                        .text_color(p.ink_3)
                        .child(kit::icon(
                            if self.ui.show_completed {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            component::ICON_SMALL,
                            p.ink_3,
                        ))
                        .child(format!(
                            "{} COMPLETED ON TIMELINE ({completed})",
                            if self.ui.show_completed {
                                "HIDE"
                            } else {
                                "SHOW"
                            }
                        )),
                    )
                })
                .when(
                    self.last_undo.is_none() && self.room_width(window) >= desktop::LIST_MIN,
                    |d| {
                        d.child(
                            div()
                                .ml_auto()
                                .truncate()
                                .style(ty::META)
                                .text_color(p.ink_3)
                                .child("Drag a bar edge to set Start or Due date"),
                        )
                    },
                ),
        )
    }
}

fn week(day: Date) -> String {
    format!("W{:02}", day.iso_week_date().week())
}
fn is_weekend(date: Date) -> bool {
    matches!(Weekday::of(date), Weekday::Sat | Weekday::Sun)
}
fn bar_inset(axis: Axis) -> f32 {
    space::S1.min(axis.day_width() / 8.)
}

fn today_x(axis: Axis, date: Date) -> f32 {
    axis.x(date) + axis.day_width() / 2.
}

fn columns(axis: Axis) -> Vec<(Date, f32, f32)> {
    let (first, last) = axis.range();
    axis.columns(first, last).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, store::Store, theme::Theme};
    use todo_core::time::add_days;

    fn fixture<'a>(
        name: &str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (gpui::Entity<ShellView>, &'a mut gpui::VisualTestContext) {
        let f = fixtures::by_name(name).expect("timeline fixture");
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
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
    fn keys_select_shift_resize_open_calendar_and_undo(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture("timeline", cx);
        let (id, original) = cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.view(), ListView::Timeline);
            let id = view.timeline_ids()[0];
            (id, Dates::of(&view.store.view().tasks[&id]))
        });
        cx.simulate_keystrokes("down alt-shift-right");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.timeline.cursor, Some(id));
            assert_eq!(
                Some(Dates::of(&view.store.view().tasks[&id])),
                Schedule::Move(7).preview(original)
            );
            assert!(view.last_undo.is_some());
        });
        cx.simulate_keystrokes("ctrl-z ]");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(
                Some(Dates::of(&view.store.view().tasks[&id])),
                Schedule::Duration(1).preview(original)
            );
        });
        cx.simulate_keystrokes("ctrl-z shift-d");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert!(matches!(view.menu.as_ref().map(|m| m.kind), Some(MenuKind::Date(c)) if c.starts && c.timeline == Some(original)));
        });
        cx.simulate_keystrokes("right enter");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(
                Dates::of(&view.store.view().tasks[&id]).start,
                original.start.and_then(|day| add_days(day, 1))
            );
        });
    }

    #[gpui::test]
    fn pointer_resizes_a_milestone_and_schedules_a_tray_card(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture("timeline-tray", cx);
        let modifiers = gpui::Modifiers::default();
        cx.simulate_mouse_move(point(px(0.), px(0.)), None, modifiers);
        let (milestone, initial, edge, to, tray_task, tray_start, tray_end, chosen) =
            cx.update(|window, cx| {
                let view = shell.read(cx);
                let (lane, _) = view.timeline_lanes().nth(1).expect("due only");
                let grid = view.timeline.grid.get();
                let axis = view.timeline_axis(window);
                let edge = point(
                    grid.left()
                        + px(axis.x(lane.span.first) + space::S1 + geometry::RESIZE_HIT / 2.),
                    grid.top() + px(geometry::LANE_ROW * 1.5),
                );
                let to = point(edge.x - px(axis.day_width()), edge.y);
                let tray = view.timeline.tray_scroll.handle.bounds();
                let tray_task = view
                    .shell
                    .room
                    .timeline
                    .as_ref()
                    .expect("timeline")
                    .unscheduled[0]
                    .id;
                let tray_start = point(
                    tray.left() + px(desktop::LIST_PAD_X + space::S3),
                    tray.top() + px(geometry::RULER_HEIGHT / 2.),
                );
                let tray_end = point(grid.center().x + px(axis.day_width() / 2.), grid.center().y);
                (
                    lane.task,
                    lane.span.first,
                    edge,
                    to,
                    tray_task,
                    tray_start,
                    tray_end,
                    axis.date_at(f32::from(tray_end.x - grid.left())),
                )
            });
        cx.simulate_mouse_move(edge, None, modifiers);
        cx.simulate_mouse_down(edge, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(to, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert_eq!(state.tasks[&milestone].due, Some(initial));
            assert_eq!(state.tasks[&milestone].start_date, add_days(initial, -1));
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.simulate_mouse_move(tray_start, None, modifiers);
        cx.simulate_mouse_down(tray_start, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(tray_end, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(tray_end, MouseButton::Left, modifiers);
        cx.update(|_, cx| {
            let state = shell.read(cx).store.view();
            assert_eq!(state.tasks[&milestone].start_date, None);
            assert_eq!(state.tasks[&tray_task].due, chosen);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(shell.read(cx).store.view().tasks[&tray_task].due, None));

        let center = cx.update(|window, cx| {
            let v = shell.read(cx);
            let axis = v.timeline_axis(window);
            let grid = v.timeline.grid.get();
            point(
                grid.left() + px(axis.x(initial) + axis.day_width() / 2.),
                grid.top() + px(geometry::LANE_ROW * 1.5),
            )
        });
        cx.simulate_mouse_move(center, None, modifiers);
        cx.simulate_mouse_down(center, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(center, MouseButton::Left, modifiers);
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, Some(milestone)));
    }

    #[gpui::test]
    fn planned_switches_to_the_shared_cross_list_projection(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = fixture("timeline-planned", cx);
        cx.update(|_, cx| assert_eq!(shell.read(cx).view(), ListView::List));
        cx.simulate_keystrokes("3");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert_eq!(view.view(), ListView::Timeline);
            assert!(
                view.timeline_lanes()
                    .any(|(l, _)| l.span.kind == SpanKind::OpenEnded)
            );
            assert_eq!(view.timeline_counts().1, 0);
            let axis = view.timeline_axis(window);
            let center = axis.date_at(axis.width / 2.);
            shell.update(cx, |view, cx| view.timeline_control(6, window, cx));
            let zoomed = shell.read(cx).timeline_axis(window);
            assert_eq!(zoomed.zoom, Zoom::Quarter);
            assert_eq!(center, zoomed.date_at(zoomed.width / 2.));
        });
    }
}
