//! Timeline viewport and input. The shared axis, projection and Schedule intents own calendar math.

use leptos::prelude::*;
use todo_client::{
    intent::{
        calendar,
        workflow::{self, Dates, Schedule},
    },
    timeline::{self, Axis, Lane, Span, SpanKind, Zoom},
};
use todo_core::{
    query::QueryStatus,
    time::{Date, Weekday, add_days, days_between},
};
use todo_design::{
    Icon,
    layout::{elevation, space, timeline as geometry},
    motion,
};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Element, HtmlElement, KeyboardEvent, PointerEvent};

use super::{
    Ctx, DueTone, RoomId, TaskId, ctx, html, icon, model, pickers, platform, quick_add,
    workflow as ui_workflow,
};

// Two extra windows on either side make the calendar natively scrollable, including with a finger.
// Toolbar jumps and revealing a distant bar rebase this bounded surface, not the tasks' dates.
const BUFFER: i64 = 2;
const WINDOWS: i64 = BUFFER * 2 + 1;
const ZOOM_KEY: &str = "todo-timeline-zoom";
const TRAY_KEY: &str = "todo-timeline-tray";
const RAIL_KEY: &str = "todo-timeline-rail";
const SCROLL: &str = "timeline-scroll";
const SURFACE: &str = "timeline-surface";
const RULER: &str = "timeline-ruler";
const RAIL: &str = "timeline-rail-toggle";
const SHORT_DAY: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Grip {
    Body,
    Start,
    Due,
    Tray,
}

#[derive(Clone)]
struct Drag {
    id: i32,
    task: TaskId,
    before: Dates,
    grip: Grip,
    captured: Element,
    x: f32,
    y: f32,
    scroll: f32,
    lifted: bool,
    current: (f32, f32),
}

#[derive(Clone, Copy)]
struct CalendarTarget {
    task: TaskId,
    dates: Dates,
    starts: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Ui {
    epoch: RwSignal<Date>,
    zoom: RwSignal<Zoom>,
    offset: RwSignal<f32>,
    width: RwSignal<f32>,
    tray: RwSignal<bool>,
    rail: RwSignal<bool>,
    cursor: RwSignal<Option<TaskId>>,
    drag: RwSignal<Option<Drag>, LocalStorage>,
    scroll_frame: StoredValue<bool>,
    suppress_click: StoredValue<bool>,
    calendar_target: RwSignal<Option<CalendarTarget>>,
    calendar: quick_add::DatePicker,
}

impl Ui {
    pub(super) fn new(today: Date) -> Self {
        let zoom: Zoom = read_pref(ZOOM_KEY).unwrap_or_default();
        let first = zoom.around_today(today);
        Self {
            epoch: RwSignal::new(first),
            zoom: RwSignal::new(zoom),
            offset: RwSignal::new(0.),
            width: RwSignal::new(0.),
            tray: RwSignal::new(read_pref(TRAY_KEY).unwrap_or(false)),
            rail: RwSignal::new(read_pref(RAIL_KEY).unwrap_or(true)),
            cursor: RwSignal::new(None),
            drag: RwSignal::new_local(None),
            scroll_frame: StoredValue::new(false),
            suppress_click: StoredValue::new(false),
            calendar_target: RwSignal::new(None),
            calendar: quick_add::DatePicker::new(
                pickers::DetailPicker::anchored(SCROLL, "timeline-calendar"),
                "timeline-active-day",
                "timeline-month-label",
            ),
        }
    }

    pub(super) fn reset(self) {
        self.cancel();
        self.calendar.popup.close(false);
        self.calendar_target.set(None);
        self.cursor.set(None);
    }

    pub(super) fn dragging(self) -> bool {
        self.drag.with_untracked(Option::is_some)
    }

    pub(super) fn cancel(self) {
        if let Some(g) = self.drag.get_untracked() {
            self.drag.set(None);
            self.suppress_click.set_value(true);
            if g.captured.has_pointer_capture(g.id) {
                let _ = g.captured.release_pointer_capture(g.id);
            }
        }
    }

    fn axis(self) -> Axis {
        Axis {
            offset: self.offset.get(),
            ..self.surface()
        }
    }

    fn surface(self) -> Axis {
        Axis {
            epoch: self.epoch.get(),
            zoom: self.zoom.get(),
            width: self.width.get().max(1.),
            offset: 0.,
        }
    }

    fn axis_now(self) -> Axis {
        untrack(|| self.axis())
    }

    // The ruler uses a sticky rail. Its measured width is the viewport inset, not part of a day.
    fn metrics(self) {
        let (Some(scroll), Some(rail), Some(surface)) =
            (element(SCROLL), element(RAIL), element(SURFACE))
        else {
            return;
        };
        let rail = rail.get_bounding_client_rect().width() as f32;
        let width = scroll.client_width() as f32 - rail;
        if width > 0. && width != self.width.get_untracked() {
            self.cancel();
            // Scroll is clamped to the current DOM width. Update CSSOM before setting scrollLeft.
            let _ = surface
                .style()
                .set_property("width", &px(rail + width * WINDOWS as f32));
            self.width.set(width);
            self.set_scroll(self.offset.get_untracked(), false);
        }
    }

    fn set_scroll(self, offset: f32, animate: bool) {
        let smooth = animate && !platform::reduced_motion();
        if !smooth {
            self.offset.set(offset);
        }
        let x = offset * self.axis_now().day_width();
        if let Some(el) = element(SCROLL) {
            let options = web_sys::ScrollToOptions::new();
            options.set_left(f64::from(x.round()));
            options.set_behavior(if smooth {
                web_sys::ScrollBehavior::Smooth
            } else {
                web_sys::ScrollBehavior::Instant
            });
            el.scroll_to_with_scroll_to_options(&options);
        }
    }

    fn navigate(self, axis: Axis, zoom: Zoom, target: f32, animate: bool) {
        self.cancel();
        let target = Axis { zoom, ..axis }.clamp(target);
        if axis.zoom == zoom && (0. ..=((WINDOWS - 1) * zoom.days()) as f32).contains(&target) {
            self.set_scroll(target, animate);
        } else {
            self.rebase(axis.epoch, zoom, target);
        }
    }

    fn rebase(self, epoch: Date, zoom: Zoom, target: f32) {
        let first = add_days(epoch, target.floor() as i64).unwrap_or(Date::MIN);
        let origin = add_days(first, -BUFFER * zoom.days())
            .unwrap_or(Date::MIN)
            .min(add_days(Date::MAX, 1 - WINDOWS * zoom.days()).unwrap_or(first));
        self.epoch.set(origin);
        self.zoom.set(zoom);
        // Zoom doesn't change the pixel width of the surface, so scroll can be restored immediately.
        self.set_scroll(
            days_between(origin, first) as f32 + target.fract().rem_euclid(1.),
            false,
        );
    }

    fn today(self, today: Date, animate: bool) {
        let axis = self.axis_now();
        self.navigate(
            axis,
            axis.zoom,
            days_between(axis.epoch, axis.zoom.around_today(today)) as f32,
            animate,
        );
    }

    fn toggle_tray(self) {
        self.cancel();
        let open = !self.tray.get_untracked();
        self.tray.set(open);
        save_pref(TRAY_KEY, &open);
    }

    fn toggle_rail(self) {
        let open = !self.rail.get_untracked();
        self.rail.set(open);
        save_pref(RAIL_KEY, &open);
        request_animation_frame(move || {
            if self.width.try_get_untracked().is_some() {
                self.metrics();
            }
        });
    }
}

// Disconnect before dropping the callback when a view or row leaves.
struct WatchSize {
    observer: web_sys::ResizeObserver,
    _callback: Closure<dyn FnMut()>,
}

impl WatchSize {
    fn new(nodes: &[&Element], update: impl FnMut() + 'static) -> Option<Self> {
        let callback = Closure::<dyn FnMut()>::new(update);
        let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
        for node in nodes {
            observer.observe(node);
        }
        Some(Self {
            observer,
            _callback: callback,
        })
    }
}

impl Drop for WatchSize {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

fn read_pref<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    let value = window().local_storage().ok()??.get_item(key).ok()??;
    serde_json::from_str(&value).ok()
}

fn save_pref(key: &str, value: &impl serde::Serialize) {
    if let Some(storage) = window().local_storage().ok().flatten()
        && let Ok(value) = serde_json::to_string(value)
    {
        let _ = storage.set_item(key, &value);
    }
}

fn element(id: &str) -> Option<HtmlElement> {
    document().get_element_by_id(id)?.dyn_into().ok()
}
fn row_id(id: TaskId) -> String {
    format!("row-{id}")
}
fn px(n: f32) -> String {
    format!("{n}px")
}
fn week(date: Date) -> String {
    format!("W{:02}", date.iso_week_date().week())
}
fn weekend(day: Date) -> bool {
    matches!(Weekday::of(day), Weekday::Sat | Weekday::Sun)
}

pub(super) fn counts(c: Ctx) -> (usize, usize, usize) {
    c.timeline.with(|t| {
        t.as_ref().map_or((0, 0, 0), |t| {
            let done = t
                .lanes
                .iter()
                .filter(|l| l.stage == QueryStatus::Done)
                .count();
            (t.lanes.len() - done, t.unscheduled.len(), done)
        })
    })
}

fn visible_lanes(c: Ctx) -> Vec<TaskId> {
    c.timeline.with(|t| {
        t.iter()
            .flat_map(|t| &t.lanes)
            .filter(|l| l.stage != QueryStatus::Done || c.completed_open.get())
            .map(|l| l.task)
            .collect()
    })
}

fn completion_phase(c: Ctx, task: TaskId) -> Option<super::Phase> {
    c.lingering.with(|rows| {
        rows.iter()
            .find(|r| {
                r.row.id == task
                    && r.board_slot.is_none()
                    && r.completion_stamp.is_some()
                    && r.room == c.room.get()
            })
            .map(|r| r.phase)
    })
}

fn tray_ids(c: Ctx) -> Vec<TaskId> {
    c.timeline.with(|t| {
        t.as_ref()
            .map(|t| t.unscheduled.clone())
            .unwrap_or_default()
    })
}

fn ids(c: Ctx) -> Vec<TaskId> {
    let mut ids = visible_lanes(c);
    if c.timeline_ui.tray.get() {
        ids.extend(tray_ids(c));
    }
    ids
}

pub(super) fn slot(c: Ctx, id: TaskId) -> Option<usize> {
    untrack(|| ids(c).iter().position(|&task| task == id))
}

pub(super) fn after_completion(c: Ctx, id: TaskId, slot: usize) {
    let visible = untrack(|| ids(c));
    if let Some(next) = visible
        .iter()
        .copied()
        .find(|&task| task == id)
        .or_else(|| visible.get(slot).copied())
        .or_else(|| visible.last().copied())
    {
        refocus(c, next);
    } else {
        c.timeline_ui.cursor.set(None);
        platform::focus(SCROLL);
    }
}

pub(super) fn refocus(c: Ctx, id: TaskId) {
    let ui = c.timeline_ui;
    let before = pickers::focused();
    ui.cursor.set(Some(id));
    request_animation_frame(move || {
        // Undo and a date shortcut can land in one frame. Do not take focus from the calendar:
        // Escape must reach its key handler so the popover can return focus to its opener.
        if c.room_view.try_get_untracked() != Some(todo_core::model::ListView::Timeline)
            || c.detail_modal.get_untracked()
            || ui.calendar.popup.open.get_untracked()
            || ui.cursor.get_untracked() != Some(id)
            || !pickers::focus_unmoved(before.as_ref())
        {
            return;
        }
        reveal(c, id, true);
    });
}

pub(super) fn clear_cursor(c: Ctx) {
    if pickers::focused().is_some_and(|el| el.closest(".timeline").ok().flatten().is_some()) {
        platform::focus(SCROLL);
        c.timeline_ui.cursor.set(None);
    }
}

pub(super) fn after_undo(c: Ctx) {
    if let Some(id) = c.timeline_ui.cursor.get_untracked()
        && pickers::focused().is_none_or(|el| {
            el.tag_name() == "BODY" || el.closest(".timeline, .toast").ok().flatten().is_some()
        })
    {
        refocus(c, id);
    }
}

fn reveal(c: Ctx, id: TaskId, focus: bool) {
    let ui = c.timeline_ui;
    if let Some(span) = c.timeline.with_untracked(|t| {
        t.as_ref()?
            .lanes
            .iter()
            .find(|l| l.task == id)
            .map(|l| l.span)
    }) {
        let axis = ui.axis_now();
        let next = axis.reveal_offset(span);
        if next != axis.offset {
            ui.navigate(axis, axis.zoom, next, false);
        }
    }
    if let Some(row) = element(&row_id(id)) {
        if focus {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = row.focus_with_options(&options);
        }
        if let Some(pane) = row
            .closest(".timeline-scroll, .timeline-tray-cards")
            .ok()
            .flatten()
        {
            let view = pane.get_bounding_client_rect();
            let row = row.get_bounding_client_rect();
            let top = if pane.id() == SCROLL {
                element(RULER).map_or(view.top(), |r| r.get_bounding_client_rect().bottom())
            } else {
                view.top()
            };
            let bottom = view.top() + f64::from(pane.client_height());
            let dy = if row.bottom() > bottom {
                row.bottom() - bottom
            } else {
                (row.top() - top).min(0.)
            };
            if dy != 0. {
                pane.set_scroll_top(pane.scroll_top() + dy.round() as i32);
            }
        }
    } else if focus {
        platform::focus(SCROLL);
    }
}

fn dates(c: Ctx, id: TaskId) -> Option<Dates> {
    if untrack(|| completion_phase(c, id).is_some()) {
        return None;
    }
    let room = c.room.get_untracked();
    c.state.with_untracked(|s| {
        s.tasks
            .get(&id)
            .filter(|t| timeline::includes(s, model::shared_key(&room), t))
            .map(Dates::of)
    })
}

fn apply(c: Ctx, id: TaskId, before: Dates, edit: Schedule) {
    if !untrack(|| c.on_timeline()) || c.read_only() {
        return;
    }
    let room = c.room.get_untracked();
    let Some((command, title)) = c.state.with_untracked(|s| {
        Some((
            workflow::schedule(s, model::shared_key(&room), id, before, edit)?,
            s.tasks.get(&id)?.title.clone(),
        ))
    }) else {
        return;
    };
    let action = if before == Dates::default() {
        "Scheduled"
    } else {
        "Rescheduled"
    };
    if c.submit_undoable(command, format!("{action} “{title}”"), None) {
        refocus(c, id);
    }
}

fn open_calendar(c: Ctx, task: TaskId, starts: bool, anchor: String) {
    let ui = c.timeline_ui;
    if !c.read_only()
        && let Some(dates) = dates(c, task)
    {
        ui.cursor.set(Some(task));
        ui.calendar_target.set(Some(CalendarTarget {
            task,
            dates,
            starts,
        }));
        ui.calendar.popup.set_anchor(anchor);
        ui.calendar.show(
            c.now.get_untracked().today(),
            if starts { dates.start } else { dates.due },
        );
    }
}

fn open_task(c: Ctx, id: TaskId) {
    c.timeline_ui.cursor.set(Some(id));
    c.select(Some(id));
    if !c.detail_modal.get_untracked() {
        refocus(c, id);
    }
}

fn click_task(c: Ctx, id: TaskId, ev: &leptos::ev::MouseEvent) {
    if ev.detail() == 0 || !c.timeline_ui.suppress_click.get_value() {
        open_task(c, id);
    }
}

pub(super) fn key(c: Ctx, ev: &KeyboardEvent, key: &str) -> bool {
    if !ev
        .target()
        .and_then(|t| t.dyn_into::<Element>().ok())
        .is_some_and(|el| {
            matches!(el.id().as_str(), "room-title" | SCROLL)
                || el
                    .matches(".timeline-task, .timeline-bar, .timeline-label, .timeline-tray-title")
                    .unwrap_or(false)
        })
    {
        return false;
    }
    let ui = c.timeline_ui;
    let ids = untrack(|| ids(c));
    let selected = ui
        .cursor
        .get_untracked()
        .or(c.selected.get_untracked())
        .filter(|id| ids.contains(id));
    let task = selected.or_else(|| ids.first().copied());
    match (key, ev.ctrl_key(), ev.alt_key(), ev.shift_key()) {
        ("arrowup" | "k" | "arrowdown" | "j", false, false, false) => {
            let next = selected.map_or(task, |id| {
                let by = if matches!(key, "arrowup" | "k") {
                    -1
                } else {
                    1
                };
                ids.iter()
                    .position(|t| *t == id)?
                    .checked_add_signed(by)
                    .and_then(|i| ids.get(i).copied())
            });
            if let Some(id) = next {
                if c.selected.get_untracked().is_some() {
                    c.preview_task(Some(id));
                }
                refocus(c, id);
            }
        }
        ("arrowleft" | "arrowright", true, true, false)
        | ("arrowleft" | "arrowright", false, true, true)
        | ("[" | "]", false, false, false) => {
            if !ev.repeat()
                && let Some(id) = task
                && let Some(before) = dates(c, id)
            {
                let by = if matches!(key, "arrowleft" | "[") {
                    -1
                } else {
                    1
                };
                let edit = if ev.alt_key() {
                    Schedule::Move(if ev.shift_key() { by * 7 } else { by })
                } else {
                    Schedule::Duration(by)
                };
                apply(c, id, before, edit);
            }
        }
        ("0", false, false, false) => {
            ui.today(c.now.get_untracked().today(), true);
        }
        ("d", false, false, starts) => {
            if let Some(id) = task {
                reveal(c, id, true);
                open_calendar(c, id, starts, row_id(id));
            }
        }
        ("enter", false, false, false) => {
            if let Some(id) = task {
                open_task(c, id);
            }
        }
        (" ", false, false, false) => {
            if !c.read_only()
                && let Some(id) = task
                && untrack(|| completion_phase(c, id).is_none())
                && let Some(row) = c
                    .state
                    .with_untracked(|s| model::board_row(s, id, c.now.get_untracked().dates()))
            {
                c.toggle(&row);
            }
        }
        _ => return false,
    }
    true
}

fn pointer_start(c: Ctx, task: TaskId, grip: Grip, ev: &PointerEvent) {
    let ui = c.timeline_ui;
    ui.suppress_click.set_value(false);
    if !ev.is_primary()
        || ev.button() != 0
        || ev.ctrl_key()
        || ev.meta_key()
        || ev.shift_key()
        || c.read_only()
        || ui.dragging()
    {
        return;
    }
    let Some((before, captured)) = dates(c, task).zip(
        ev.current_target()
            .and_then(|t| t.dyn_into::<Element>().ok()),
    ) else {
        return;
    };
    let id = ev.pointer_id();
    if captured.set_pointer_capture(id).is_err() {
        return;
    }
    let x = ev.client_x() as f32;
    let y = ev.client_y() as f32;
    let axis = ui.axis_now();
    ui.set_scroll(axis.offset, false);
    ui.cursor.set(Some(task));
    ui.drag.set(Some(Drag {
        id,
        task,
        before,
        grip,
        captured,
        x,
        y,
        scroll: axis.offset,
        lifted: false,
        current: (x, y),
    }));
}

/// Horizontal grid bounds when the lifted pointer is inside the date surface.
fn drag_bounds(g: &Drag) -> Option<(f32, f32)> {
    let (x, y) = g.current;
    let scroll = element(SCROLL)?;
    let rect = scroll.get_bounding_client_rect();
    let left = element(RAIL)?.get_bounding_client_rect().right() as f32;
    let right = rect.left() as f32 + scroll.client_width() as f32;
    let top = element(RULER)?.get_bounding_client_rect();
    let top = if g.grip == Grip::Tray {
        top.top()
    } else {
        top.bottom()
    };
    (g.lifted
        && (left..right).contains(&x)
        && (top as f32..rect.top() as f32 + scroll.client_height() as f32).contains(&y))
    .then_some((left, right))
}

fn drag_edit(ui: Ui, g: &Drag) -> Option<Schedule> {
    let (left, _) = drag_bounds(g)?;
    let x = g.current.0;
    let axis = ui.axis();
    let by = axis.days_dragged(x - g.x + (axis.offset - g.scroll) * axis.day_width());
    Some(match g.grip {
        Grip::Body => Schedule::Move(by),
        Grip::Start => Schedule::StartEdge(by),
        Grip::Due => Schedule::DueEdge(by),
        Grip::Tray => Schedule::Place(axis.date_at(x - left)?),
    })
}

fn pointer_move(ui: Ui, ev: &PointerEvent) {
    let Some(mut g) = ui.drag.get_untracked().filter(|g| g.id == ev.pointer_id()) else {
        return;
    };
    g.current = (ev.client_x() as f32, ev.client_y() as f32);
    g.lifted |= (g.current.0 - g.x).abs().max((g.current.1 - g.y).abs()) >= space::S1;
    if g.lifted {
        ui.suppress_click.set_value(true);
    }
    ui.drag.set(Some(g));
    drive_edge_scroll(ui, platform::clock_ms());
}

fn drive_edge_scroll(ui: Ui, previous: f64) {
    let direction = |g: &Drag| {
        let (left, right) = drag_bounds(g)?;
        let x = g.current.0;
        let edge = space::S6.min((right - left) / 2.);
        let speed = ((x - (right - edge)).max(0.) - ((left + edge) - x).max(0.)) / edge;
        (speed != 0.).then_some(speed)
    };
    if ui.scroll_frame.get_value()
        || ui
            .drag
            .with_untracked(|g| g.as_ref().and_then(direction))
            .is_none()
    {
        return;
    }
    ui.scroll_frame.set_value(true);
    request_animation_frame(move || {
        if ui.scroll_frame.try_with_value(|_| ()).is_none() {
            return;
        }
        ui.scroll_frame.set_value(false);
        let Some(speed) = ui.drag.with_untracked(|g| g.as_ref().and_then(direction)) else {
            return;
        };
        let a = ui.axis_now();
        let now = platform::clock_ms();
        let delta = ((now - previous).clamp(0., f64::from(motion::HOVER))
            / f64::from(motion::PANEL)) as f32;
        let next = a
            .clamp(a.offset + speed * delta)
            .clamp(0., ((WINDOWS - 1) * a.zoom.days()) as f32);
        if next != a.offset {
            ui.set_scroll(next, false);
            drive_edge_scroll(ui, now);
        }
    });
}

fn pointer_end(c: Ctx, ev: &PointerEvent) {
    let ui = c.timeline_ui;
    if !ui
        .drag
        .with_untracked(|g| g.as_ref().is_some_and(|g| g.id == ev.pointer_id()))
    {
        return;
    }
    pointer_move(ui, ev);
    if let Some(g) = ui.drag.get_untracked() {
        ui.drag.set(None);
        if let Some(edit) = untrack(|| drag_edit(ui, &g)) {
            apply(c, g.task, g.before, edit);
        }
    }
}

fn pointer_cancel(ui: Ui, ev: &PointerEvent) {
    if ui
        .drag
        .with_untracked(|g| g.as_ref().is_some_and(|g| g.id == ev.pointer_id()))
    {
        ui.cancel();
    }
}

#[component]
pub(super) fn Pane() -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let scroll = NodeRef::<html::Div>::new();
    // Watch the rail as well as the scroller: the responsive rail may change without a window resize.
    let observer = StoredValue::new_local(None::<WatchSize>);
    scroll.on_load(move |el| {
        ui.width.set(0.);
        ui.metrics();
        let today = c.now.get_untracked().today();
        let z = ui.zoom.get_untracked();
        ui.rebase(z.around_today(today), z, 0.);
        if let Some(rail) = element(RAIL) {
            observer.set_value(WatchSize::new(&[el.as_ref(), rail.as_ref()], move || {
                ui.metrics()
            }));
        }
    });
    on_cleanup(move || {
        observer.try_update_value(|value| {
            value.take();
        });
        ui.reset();
    });
    Effect::new(move |previous: Option<RoomId>| {
        let room = c.room.get();
        if previous.as_ref().is_some_and(|old| *old != room)
            && let Some(el) = scroll.get()
        {
            el.set_scroll_top(0);
            let z = ui.zoom.get_untracked();
            ui.rebase(z.around_today(c.now.get_untracked().today()), z, 0.);
        }
        room
    });
    Effect::new(move |_| {
        let current = |id, before| {
            !c.read_only()
                && c.state.with(|s| {
                    s.tasks.get(&id).is_some_and(|task| {
                        Dates::of(task) == before
                            && timeline::includes(s, model::shared_key(&c.room.get()), task)
                    })
                })
        };
        if ui.calendar.popup.open.get()
            && ui
                .calendar_target
                .get()
                .is_none_or(|t| !current(t.task, t.dates))
        {
            ui.calendar.popup.close(false);
        }
        if ui
            .drag
            .with(|g| g.as_ref().is_some_and(|g| !current(g.task, g.before)))
        {
            ui.cancel();
        }
    });
    let starts = Signal::derive(move || ui.calendar_target.get().is_some_and(|t| t.starts));
    let repeats = Signal::derive(move || {
        ui.calendar_target.get().is_some_and(|t| {
            c.state
                .with(|s| s.tasks.get(&t.task).is_some_and(|t| t.recurrence.is_some()))
        }) && !starts.get()
    });
    let choose = Callback::new(move |day| {
        let Some(t) = ui.calendar_target.get_untracked() else {
            return;
        };
        let edit = if t.starts {
            Schedule::PickStart(day)
        } else {
            Schedule::PickDue(day)
        };
        // Close before the edit queues refocus, so the focus snapshot is the opener, not a day.
        // If the edit moves a task out of the tray, refocus finds its new rail button by ID.
        ui.calendar.popup.close(true);
        apply(c, t.task, t.dates, edit);
    });
    view! {
        <section class="timeline" aria-label="Timeline"
            on:pointermove=move |ev| pointer_move(ui, &ev)
            on:pointerup=move |ev| pointer_end(c, &ev)
            on:pointercancel=move |ev| pointer_cancel(ui, &ev)
            on:lostpointercapture=move |ev| pointer_cancel(ui, &ev)>
            <Toolbar/>
            <div class="timeline-scroll" id=SCROLL node_ref=scroll tabindex="0" aria-label="Task schedule"
                class:rail-collapsed=move || !ui.rail.get()
                class:drag-active=move || ui.drag.with(|g| g.as_ref().is_some_and(|g| g.lifted))
                on:scroll=move |_| {
                    if let Some(el) = scroll.get_untracked() {
                        let a = ui.axis_now();
                        let offset = el.scroll_left() as f32 / a.day_width();
                        // Ignore subpixel rounding at exact day boundaries.
                        let snapped = offset.round();
                        ui.offset.set(if (snapped - offset).abs() * a.day_width() <= 0.5 { snapped } else { offset });
                        ui.calendar.popup.position();
                    }
                }>
                <div class="timeline-surface" id=SURFACE>
                    <Ruler/>
                    <div class="timeline-lanes" id="timeline-lanes">
                        <DateStrip ruler=false/>
                        <For each=move || visible_lanes(c) key=|id| *id let:id><TaskLane id=id/></For>
                        <Show when=move || visible_lanes(c).is_empty()>
                            <p class="timeline-empty">{move || if c.filter_text.with(|t| !t.trim().is_empty()) { "Nothing matches the filter." } else { "No scheduled tasks." }}</p>
                        </Show>
                        <div class="timeline-sticky timeline-rail-tail" aria-hidden="true"></div>
                    </div>
                </div>
            </div>
            <Tray/>
            <DragReadout/>
            <quick_add::CalendarPopover c=c picker=ui.calendar choose=choose starts=starts repeats=repeats/>
        </section>
    }
}

#[component]
fn Toolbar() -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let step = move |by: i64| {
        let a = ui.axis_now();
        ui.navigate(a, a.zoom, a.offset + (by * a.zoom.days()) as f32, true);
    };
    let count = Memo::new(move |_| tray_ids(c).len());
    view! {
        <div class="timeline-toolbar" role="group" aria-label="Timeline controls">
            <div class="timeline-nav">
                <button type="button" class="timeline-control" aria-label="Previous dates" title="Previous dates" on:click=move |_| step(-1)>{icon(Icon::ChevronLeft)}</button>
                <button type="button" class="timeline-control" title="Jump to Today (0)" on:click=move |_| ui.today(c.now.get_untracked().today(), true)>"Today"</button>
                <button type="button" class="timeline-control" aria-label="Next dates" title="Next dates" on:click=move |_| step(1)>{icon(Icon::ChevronRight)}</button>
            </div>
            <span class="timeline-range">{move || {
                let (first, last) = ui.axis().range();
                format!("{} · {}–{}", c.now.get().dates().range(first, last), week(first), week(last))
            }}</span>
            <div class="view-switcher timeline-zoom" role="group" aria-label="Timeline zoom">
                {Zoom::ALL.into_iter().zip([("Week", "1W", "Week"), ("2W", "2W", "Two weeks"), ("Month", "M", "Month"), ("Quarter", "Q", "Quarter")]).map(|(z, (wide, short, name))| view! {
                    <button type="button" aria-label=name title=name aria-pressed=move || (ui.zoom.get() == z).to_string()
                        on:click=move |_| {
                            let a = ui.axis_now();
                            if z != a.zoom {
                                ui.navigate(a, z, a.zoom_offset(z), false);
                                save_pref(ZOOM_KEY, &z);
                            }
                        }><span class="timeline-long">{wide}</span><span class="timeline-short">{short}</span></button>
                }).collect_view()}
            </div>
            <Show when=move || count.get() != 0>
                <button type="button" class="timeline-control timeline-tray-button" aria-label=move || format!("Unscheduled tasks, {}", count.get())
                    aria-expanded=move || ui.tray.get().to_string() aria-controls="timeline-tray-cards" title="Unscheduled tasks"
                    on:click=move |_| ui.toggle_tray()>
                    {icon(Icon::Calendar)}<span class="timeline-long">"Unscheduled · "</span>{move || count.get()}
                </button>
            </Show>
        </div>
    }
}

#[component]
fn Ruler() -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    view! {
        <div class="timeline-ruler" id=RULER>
            <div class="timeline-sticky timeline-ruler-rail" id=RAIL>
                <button type="button" class="timeline-control timeline-rail-button" aria-expanded=move || ui.rail.get().to_string()
                    aria-label=move || if ui.rail.get() { "Collapse task rail" } else { "Expand task rail" }
                    title=move || if ui.rail.get() { "Collapse task rail" } else { "Expand task rail" }
                    on:click=move |_| ui.toggle_rail()>
                    <span class="timeline-rail-name">"Task"</span>
                    {move || icon(if ui.rail.get() { Icon::ChevronLeft } else { Icon::ChevronRight })}
                </button>
                <span class="caps timeline-rail-count">{move || format!("{} shown", visible_lanes(c).len())}</span>
            </div>
            <DateStrip ruler=true/>
        </div>
    }
}

#[component]
fn DateStrip(ruler: bool) -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    view! {
        <div class="timeline-date-strip" aria-hidden=(!ruler).then_some("true")>
            {move || {
                let a = ui.surface();
                let today = c.now.get().today();
                let last = add_days(a.epoch, WINDOWS * a.zoom.days() - 1).unwrap_or(Date::MAX);
                a.columns(a.epoch, last).map(|(day, x, width)| {
                    let quarter = a.zoom == Zoom::Quarter;
                    let current = day <= today && days_between(day, today) < a.zoom.column_days();
                    let day_name = SHORT_DAY[Weekday::of(day).offset()];
                    let top = if quarter { week(day) } else if a.zoom == Zoom::Month { day_name[..1].into() } else { day_name.into() };
                    let bottom = if quarter || day.day() == 1 && a.zoom != Zoom::Month { c.now.get().dates().date(day) } else { day.day().to_string() };
                    view! {
                        <div class="timeline-date-cell" class:today=current class:weekend=!quarter && weekend(day)
                            style:left=px(x) style:width=px(width) data-date=ruler.then(|| day.to_string())
                            title=ruler.then(|| calendar::day_label(day)) aria-current=(ruler && current).then_some("date")
                            aria-hidden=move || (!ruler || {
                                let axis = ui.axis(); let left = axis.offset * axis.day_width();
                                x + width <= left || x >= left + axis.width
                            }).then_some("true")>
                            {ruler.then(|| view! {
                                <span class="timeline-iso" hidden=move || quarter || (Weekday::of(day) != Weekday::Mon && ui.axis().range().0 != day)>{week(day)}</span>
                                <span class="timeline-weekday" data-initial=&day_name[..1] class:full=quarter>{top}</span>
                                <span class="timeline-day">{bottom}</span>
                            })}
                        </div>
                    }
                }).collect_view()
            }}
            <i class=if ruler { "timeline-today-dot" } else { "timeline-today-line" } aria-hidden="true"
                hidden=move || {
                    let a = ui.surface(); let x = a.x(c.now.get().today()) + a.day_width() / 2.;
                    !(0. ..a.width * WINDOWS as f32).contains(&x)
                }
                style:left=move || { let a = ui.surface(); px(a.x(c.now.get().today()) + a.day_width() / 2.) }></i>
        </div>
    }
}

#[derive(Clone, PartialEq)]
struct TaskPaint {
    row: model::Row,
    hue: &'static str,
    list: String,
    stage: QueryStatus,
}

fn task_paint(c: Ctx, id: TaskId) -> Option<TaskPaint> {
    c.state.with(|s| {
        let task = s.tasks.get(&id)?;
        let list = s.lists.get(&task.list)?;
        let stage = if list.features.workflow || task.is_done() {
            QueryStatus::of(task)
        } else {
            QueryStatus::Todo
        };
        Some(TaskPaint {
            row: model::board_row(s, id, c.now.get().dates())?,
            list: list.name.clone(),
            hue: model::swatch(list.color.as_deref(), list.id.as_uuid().as_bytes()),
            stage,
        })
    })
}

#[derive(Clone, Copy, PartialEq)]
struct Bar {
    span: Span,
    left: f32,
    right: f32,
    start_grip: bool,
    due_grip: bool,
    inline: bool,
    label_right: bool,
    label_width: f32,
}

fn bar(ui: Ui, lane: Lane, label: f32, selected: bool) -> Option<Bar> {
    let span = ui
        .drag
        .with(|g| {
            g.as_ref()
                .filter(|g| g.task == lane.task)
                .and_then(|g| drag_edit(ui, g)?.preview(g.before))
                .and_then(|dates| Span::from_dates(dates.start, dates.due))
        })
        .unwrap_or(lane.span);
    let a = ui.axis();
    let view_left = a.offset * a.day_width();
    let view_right = view_left + a.width;
    let natural_left = a.x(span.first) + view_left;
    let natural_right = a.x(span.last) + view_left + a.day_width();
    if natural_right <= view_left || natural_left >= view_right {
        return None;
    }
    let inset = space::S1.min(a.day_width() / 8.);
    let left = (natural_left + inset).max(view_left);
    let right = (natural_right - inset)
        .min(view_right)
        .max(left + elevation::HAIRLINE);
    let width = right - left;
    let after = (view_right - right - space::S2).max(0.);
    let before = (left - view_left - space::S2).max(0.);
    let label_right = after >= label + 2. * space::S1 || after >= before;
    Some(Bar {
        span,
        left,
        right,
        start_grip: span.kind != SpanKind::OpenEnded && natural_left >= view_left,
        due_grip: span.kind != SpanKind::Milestone && natural_right <= view_right,
        inline: span.kind != SpanKind::Milestone
            && width >= geometry::INLINE_LABEL_MIN
            && (selected
                || width >= label + 2. * (geometry::RESIZE_HIT + space::S1)
                || after.max(before) <= width),
        label_right,
        label_width: if label_right { after } else { before },
    })
}

#[component]
fn TaskLane(id: TaskId) -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let task = Memo::new(move |_| task_paint(c, id));
    let lane = Memo::new(move |_| {
        c.timeline
            .with(|t| t.as_ref()?.lanes.iter().find(|l| l.task == id).copied())
    });
    let cross = move || c.room.with(|r| !matches!(r, RoomId::List(_)));
    let selected = move || ui.cursor.get().or(c.selected.get()) == Some(id);
    let probe = NodeRef::<html::Span>::new();
    let label_width = RwSignal::new(0.);
    let observer = StoredValue::new_local(None::<WatchSize>);
    probe.on_load(move |el| {
        let measure = el.clone();
        observer.set_value(WatchSize::new(&[el.as_ref()], move || {
            label_width.set(measure.get_bounding_client_rect().width() as f32);
        }));
    });
    on_cleanup(move || {
        observer.try_update_value(|value| {
            value.take();
        });
    });
    let bar = Memo::new(move |_| bar(ui, lane.get()?, label_width.get(), selected()));
    let title = move || task.with(|t| t.as_ref().map(|t| t.row.title.clone()).unwrap_or_default());
    let stage = move || {
        let stage = lane.with(|l| l.map_or(QueryStatus::Todo, |l| l.stage));
        if c.room_features.get().workflow || stage == QueryStatus::Done {
            stage
        } else {
            QueryStatus::Todo
        }
    };
    let completion = Memo::new(move |_| completion_phase(c, id));
    let hue = move || task.with(|t| t.as_ref().map(|t| t.hue));
    let overdue_label = move || {
        task.with(|t| {
            t.as_ref()
                .filter(|t| !t.row.done)
                .and_then(|t| t.row.due.as_ref())
                .filter(|d| d.tone == DueTone::Overdue)
                .map(|d| d.text.clone())
        })
    };
    let overdue = move || overdue_label().is_some();
    let steps = move || {
        task.with(|t| {
            t.as_ref()
                .and_then(|t| t.row.steps)
                .map(|(d, n)| format!("{d}/{n}"))
        })
    };
    let agent = move || {
        task.with(|t| {
            t.as_ref()
                .and_then(|t| t.row.agent.as_ref())
                .map(super::mark)
        })
    };
    let description = move || {
        let span = bar
            .get()
            .map(|b| b.span)
            .or_else(|| lane.get().map(|l| l.span));
        let dates = span.map_or_else(String::new, |s| c.now.get().dates().range(s.first, s.last));
        let list = if cross() {
            task.with(|t| {
                t.as_ref()
                    .map_or_else(String::new, |t| format!(" · {}", t.list))
            })
        } else {
            String::new()
        };
        format!(
            "{}{list} · {} · {dates}",
            title(),
            ui_workflow::stage_info(stage()).0
        )
    };
    view! {
        <div class="timeline-lane" class:done=move || stage() == QueryStatus::Done
            class:completing=move || completion.get() == Some(super::Phase::Hold)
            class:exiting=move || completion.get() == Some(super::Phase::Exit)
            class:selected=selected class:cross-list=cross class:overdue=overdue
            data-stage=move || ui_workflow::stage_info(stage()).1 data-list-hue=hue>
            <button type="button" class="timeline-sticky timeline-task" id=row_id(id)
                aria-current=move || selected().then_some("true") aria-label=title title=description
                on:focus=move |_| ui.cursor.set(Some(id)) on:pointerdown=move |_| ui.suppress_click.set_value(false)
                on:click=move |ev| click_task(c, id, &ev)>
                <Show when=move || completion.get().is_some() fallback=move || view! {
                    <Show when=cross fallback=move || view! { <span class="timeline-stage" title=move || ui_workflow::stage_info(stage()).0>{move || icon(ui_workflow::stage_info(stage()).2)}</span> }>
                        <i class="timeline-list-dot" aria-hidden="true"></i>
                    </Show>
                }>
                    <span class="check timeline-completion" aria-hidden="true">{super::tick_svg()}</span>
                </Show>
                <span class="timeline-task-title">{title}</span>
            </button>
            <div class="timeline-lane-dates">
                <span class="timeline-probe" aria-hidden="true" node_ref=probe>
                    <span>{title}</span>
                    {move || agent().map(IntoAny::into_any).or_else(|| steps().map(|step| view! { <span class="timeline-steps">{step}</span> }.into_any()))}
                </span>
                <button type="button" tabindex="-1" class="timeline-bar" class:milestone=move || bar.get().is_some_and(|b| b.span.kind == SpanKind::Milestone)
                    class:open-ended=move || bar.get().is_some_and(|b| b.span.kind == SpanKind::OpenEnded && b.due_grip)
                    class:lifted=move || ui.drag.with(|g| g.as_ref().is_some_and(|g| g.task == id && g.lifted))
                    aria-label=description title=description hidden=move || bar.with(Option::is_none)
                    data-task=id.to_string() data-start=move || dates_attr(c, id, true) data-due=move || dates_attr(c, id, false)
                    style:left=move || px(bar.get().map_or(0., |b| b.left))
                    style:width=move || px(bar.get().map_or(0., |b| b.right - b.left))
                    on:pointerdown=move |ev: PointerEvent| {
                        let grip = ev.target().and_then(|el| el.dyn_into::<Element>().ok())
                            .and_then(|el| el.get_attribute("data-edge")).map_or(Grip::Body, |g| if g == "start" { Grip::Start } else { Grip::Due });
                        pointer_start(c, id, grip, &ev);
                    }
                    on:click=move |ev| click_task(c, id, &ev)>
                    <span class="timeline-inline" hidden=move || !bar.get().is_some_and(|b| b.inline)>{title}</span>
                    <Show when=move || bar.get().is_some_and(|b| b.span.kind == SpanKind::Milestone)>
                        <span class="timeline-milestone" aria-hidden="true">{move || if overdue() { "◆".into_any() } else { icon(Icon::Flag).into_any() }}</span>
                    </Show>
                    {move || {
                        let b = bar.get()?;
                        let mark = b.inline.then(agent).flatten().map(IntoAny::into_any);
                        mark.or_else(|| {
                            (b.span.kind != SpanKind::Milestone && b.right - b.left >= space::S8)
                                .then(steps).flatten().map(|step| view! { <span class="timeline-steps">{step}</span> }.into_any())
                        })
                    }}
                    <span aria-hidden="true" class="timeline-resize start" data-edge="start" title="Drag to change start date"
                        hidden=move || c.read_only() || !bar.get().is_some_and(|b| b.start_grip)></span>
                    <span aria-hidden="true" class="timeline-resize due" data-edge="due" title="Drag to change due date"
                        hidden=move || c.read_only() || !bar.get().is_some_and(|b| b.due_grip)></span>
                </button>
                <button type="button" tabindex="-1" class="timeline-label" aria-label=title title=description
                    hidden=move || !bar.get().is_some_and(|b| !b.inline && b.label_width > 0.)
                    style:left=move || px(bar.get().map_or(0., |b| if b.label_right { b.right } else { b.left }))
                    style:max-width=move || px(bar.get().map_or(0., |b| b.label_width))
                    class:before=move || bar.get().is_some_and(|b| !b.label_right)
                    on:pointerdown=move |_| ui.suppress_click.set_value(false) on:click=move |ev| click_task(c, id, &ev)>
                    {title}<span class="timeline-label-dates">{move || bar.get().map(|b| {
                        let detail = overdue_label().unwrap_or_else(|| c.now.get().dates().range(b.span.first, b.span.last));
                        format!(" · {detail}")
                    })}</span>
                </button>
            </div>
        </div>
    }
}

fn dates_attr(c: Ctx, id: TaskId, starts: bool) -> Option<String> {
    c.state.with(|s| {
        s.tasks
            .get(&id)
            .and_then(|t| if starts { t.start_date } else { t.due })
            .map(|d| d.to_string())
    })
}

#[component]
fn Tray() -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let count = Memo::new(move |_| tray_ids(c).len());
    view! {
        <Show when=move || count.get() != 0>
            <div class="timeline-tray" class:open=move || ui.tray.get()>
                <button type="button" class="timeline-tray-head caps" aria-label=move || format!("Unscheduled, {} tasks", count.get())
                    aria-expanded=move || ui.tray.get().to_string() aria-controls="timeline-tray-cards"
                    on:click=move |_| ui.toggle_tray()>
                    <span>{move || format!("Unscheduled · {}", count.get())}<span class="timeline-tray-hint" hidden=move || !ui.tray.get()>" — drag onto a date to schedule"</span></span>
                    <span class="timeline-tray-disclosure">{move || if ui.tray.get() { "Hide tray" } else { "Show tray" }}
                        {move || icon(if ui.tray.get() { Icon::ChevronDown } else { Icon::ChevronUp })}</span>
                </button>
                <div id="timeline-tray-cards" class="timeline-tray-cards" inert=move || !ui.tray.get()>
                    <For each=move || tray_ids(c) key=|id| *id let:id><TrayCard id=id/></For>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn TrayCard(id: TaskId) -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let task = Memo::new(move |_| task_paint(c, id));
    let anchor = StoredValue::new(format!("schedule-{id}"));
    let title = move || task.with(|t| t.as_ref().map(|t| t.row.title.clone()).unwrap_or_default());
    view! {
        <div class="timeline-tray-card" class:selected=move || ui.cursor.get() == Some(id)
            on:pointerdown=move |ev: PointerEvent| {
                if !ev.target().and_then(|el| el.dyn_into::<Element>().ok())
                    .is_some_and(|el| el.closest(".timeline-schedule").ok().flatten().is_some()) {
                    pointer_start(c, id, Grip::Tray, &ev);
                }
            }
            on:click=move |ev| click_task(c, id, &ev)>
            <span class="timeline-grip">{icon(Icon::GripVertical)}</span>
            {move || task.with(|t| t.as_ref().map(|t| icon(ui_workflow::stage_info(t.stage).2)))}
            <button type="button" id=row_id(id) class="timeline-tray-title" title=title
                on:focus=move |_| ui.cursor.set(Some(id))>{title}</button>
            {move || task.with(|t| t.as_ref().and_then(|t| t.row.tags.first().cloned()).map(|tag| view! { <span class="timeline-tray-tag">{tag}</span> }))}
            {move || task.with(|t| t.as_ref().and_then(|t| t.row.agent.as_ref()).map(super::mark))}
            <button type="button" class="timeline-control timeline-schedule" id=anchor.get_value() data-calendar-anchor=""
                aria-haspopup="dialog" aria-controls="timeline-calendar" aria-label=move || format!("Schedule {}", title()) disabled=move || c.read_only()
                on:click=move |ev| { ev.stop_propagation(); open_calendar(c, id, false, anchor.get_value()); }>"Schedule"</button>
        </div>
    }
}

#[component]
fn DragReadout() -> impl IntoView {
    let c = ctx();
    let ui = c.timeline_ui;
    let preview = Memo::new(move |_| {
        ui.drag.with(|g| {
            let g = g.as_ref()?;
            let dates = drag_edit(ui, g)?.preview(g.before)?;
            let span = Span::from_dates(dates.start, dates.due)?;
            let range = c.now.get().dates().range(span.first, span.last);
            let label = match span.kind {
                SpanKind::Milestone => format!("Due {range}"),
                SpanKind::OpenEnded => format!("Starts {range}"),
                SpanKind::Range => {
                    format!("{range} · {}d", days_between(span.first, span.last) + 1)
                }
            };
            Some((g.current, label))
        })
    });
    view! {
        <output class="timeline-drag-readout" role="tooltip" hidden=move || preview.with(Option::is_none)
            style:left=move || px(preview.with(|g| g.as_ref().map_or(0., |g| g.0.0)))
            style:top=move || px(preview.with(|g| g.as_ref().map_or(0., |g| g.0.1)))>
            {move || preview.with(|g| g.as_ref().map(|g| g.1.clone()))}
        </output>
    }
}
