//! Pointer-captured task drags. File drops use the independent native DragEvent path.

use super::{Ctx, ctx, document, icon, model::RoomId, platform};
use leptos::prelude::*;
use todo_client::task_drag::{Destination, Drag, Slot};
use todo_core::ids::{ListId, TaskId};
use todo_design::{Icon, layout::space};
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, PointerEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Destination(Destination),
    Slot(Slot),
}

#[derive(Clone)]
pub(super) struct Gesture {
    drag: Drag,
    room: RoomId,
    title: String,
    pointer: i32,
    touch: bool,
    x: i32,
    y: i32,
    start: (i32, i32),
    active: bool,
    opened_drawer: bool,
    target: Option<Target>,
}

impl Ctx {
    pub(super) fn drag_active(self) -> bool {
        self.task_drag
            .with(|g| g.as_ref().is_some_and(|g| g.active))
    }

    pub(super) fn drop_target(self, target: Target) -> bool {
        self.task_drag
            .with(|g| g.as_ref().is_some_and(|g| g.target == Some(target)))
    }

    pub(super) fn dragging_row(self, id: TaskId) -> bool {
        self.task_drag.with(|g| {
            g.as_ref()
                .is_some_and(|g| g.active && g.drag.ids().any(|i| i == id))
        })
    }

    pub(super) fn cancel_task_drag(self) -> bool {
        let Some(g) = self.task_drag.try_update(Option::take).flatten() else {
            return false;
        };
        if g.opened_drawer {
            self.drawer.set(false);
        }
        g.active
    }

    fn open_order(self) -> Vec<TaskId> {
        self.shown.with_untracked(|rows| {
            rows.iter()
                .filter(|s| !s.row.done && s.phase.is_none())
                .map(|s| s.row.id)
                .collect()
        })
    }

    fn manual_list(self) -> Option<ListId> {
        match self.room.get_untracked() {
            RoomId::List(list) => Some(list),
            _ => None,
        }
    }

    fn take_target(self, g: &Gesture) -> Option<Target> {
        let el = document().element_from_point(g.x as f32, g.y as f32)?;
        if let Some(nav) = el.closest("[data-task-destination]").ok().flatten() {
            let dest = match nav.get_attribute("data-task-destination")?.as_str() {
                "my-day" => Destination::MyDay,
                list => Destination::List(list.parse().ok()?),
            };
            let today = self.now.get_untracked().today();
            return self
                .state
                .with_untracked(|s| g.drag.eligible(s, dest, today))
                .then_some(Target::Destination(dest));
        }
        let list = self.manual_list()?;
        let row = el
            .closest(".task-list-area .rows:not(.completed-rows) > [data-task-row]")
            .ok()
            .flatten()?;
        let rect = row.get_bounding_client_rect();
        let slot = Slot {
            row: row.get_attribute("data-task-row")?.parse().ok()?,
            after: f64::from(g.y) >= rect.top() + rect.height() * 0.5,
        };
        self.state
            .with_untracked(|s| g.drag.placement(s, Some(list), &self.open_order(), slot))
            .map(|_| Target::Slot(slot))
    }

    fn start_drag(self, id: TaskId, title: String, ev: &PointerEvent) {
        if ev.button() != 0
            || !ev.is_primary()
            || self.read_only()
            || self.detail_modal.get_untracked()
        {
            return;
        }
        let selected = untrack(|| self.bulk_ids());
        let Some(drag) = self.state.with_untracked(|s| Drag::start(s, id, &selected)) else {
            return;
        };
        let Some(el) = ev
            .current_target()
            .and_then(|el| el.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        if el.set_pointer_capture(ev.pointer_id()).is_err() {
            return;
        }
        ev.prevent_default();
        self.task_drag.set(Some(Gesture {
            drag,
            room: self.room.get_untracked(),
            title,
            pointer: ev.pointer_id(),
            touch: ev.pointer_type() == "touch",
            x: ev.client_x(),
            y: ev.client_y(),
            start: (ev.client_x(), ev.client_y()),
            active: false,
            opened_drawer: false,
            target: None,
        }));
    }

    fn move_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .task_drag
            .get_untracked()
            .filter(|g| g.pointer == ev.pointer_id())
        else {
            return;
        };
        g.x = ev.client_x();
        g.y = ev.client_y();
        if !g.active && (g.x - g.start.0).abs().max((g.y - g.start.1).abs()) < space::S1 as i32 {
            return;
        }
        let first = !g.active;
        g.active = true;
        if !self.drawer.get_untracked()
            && g.x <= space::S5 as i32
            && g.x >= 0
            && window()
                .match_media("(max-width: 759px)")
                .ok()
                .flatten()
                .is_some_and(|m| m.matches())
        {
            g.opened_drawer = true;
            self.drawer.set(true);
        }
        g.target = self.take_target(&g);
        self.task_drag.set(Some(g));
        if first {
            request_animation_frame(move || self.scroll_drag());
        }
    }

    fn scroll_drag(self) {
        let Some(mut g) = self
            .task_drag
            .try_get_untracked()
            .flatten()
            .filter(|g| g.active)
        else {
            return;
        };
        let el = document().element_from_point(g.x as f32, g.y as f32);
        if let Some(scroll) = el.and_then(|e| e.closest(".side-body, .room").ok().flatten()) {
            let rect = scroll.get_bounding_client_rect();
            let y = f64::from(g.y);
            let edge = f64::from(space::S8);
            let step = |gap: f64| {
                ((edge - gap).clamp(0.0, edge) / edge * f64::from(space::S3)).ceil() as i32
            };
            let delta = step(rect.bottom() - y) - step(y - rect.top());
            if delta != 0 {
                scroll.set_scroll_top(scroll.scroll_top() + delta);
            }
        }
        let target = self.take_target(&g);
        if g.target != target {
            g.target = target;
            self.task_drag.set(Some(g));
        }
        request_animation_frame(move || self.scroll_drag());
    }

    fn end_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .task_drag
            .get_untracked()
            .filter(|g| g.pointer == ev.pointer_id())
        else {
            return;
        };
        g.x = ev.client_x();
        g.y = ev.client_y();
        let target = self.take_target(&g);
        self.cancel_task_drag();
        if !g.active || self.read_only() || self.room.get_untracked() != g.room {
            return;
        }
        let Some(target) = target else { return };
        let open = self.open_order();
        let today = self.now.get_untracked().today();
        let plan = self.state.with_untracked(|s| match target {
            Target::Destination(d) => g.drag.drop_on(s, d, today),
            Target::Slot(slot) => g.drag.drop_in(s, self.manual_list(), &open, slot),
        });
        self.apply_drag(
            plan,
            g.drag.source,
            matches!(target, Target::Slot(_)),
            g.room,
            true,
        );
    }

    pub(super) fn nudge_task(self, down: bool, modal: bool) {
        let RoomId::List(list) = self.room.get_untracked() else {
            return;
        };
        let selected = untrack(|| self.bulk_ids());
        let id = self
            .multi
            .with_untracked(|s| s.cursor())
            .filter(|i| selected.contains(i))
            .or_else(|| selected.first().copied())
            .or_else(|| self.selected.get_untracked());
        let Some(id) = id else { return };
        let open = self.open_order();
        let plan = self.state.with_untracked(|s| {
            Drag::start(s, id, &selected).map_or(Ok(None), |drag| drag.nudge(s, list, &open, down))
        });
        self.apply_drag(plan, id, true, RoomId::List(list), !modal);
    }

    fn apply_drag(
        self,
        plan: Result<Option<(todo_core::Command, String)>, todo_client::selection::BulkLimit>,
        source: TaskId,
        reorder: bool,
        room: RoomId,
        focus: bool,
    ) {
        match plan {
            Ok(Some((cmd, label))) => {
                let positions = if reorder {
                    platform::row_positions()
                } else {
                    Vec::new()
                };
                if self.submit_undoable(cmd, label, None) {
                    if !reorder {
                        self.multi.update(todo_client::selection::Selection::clear);
                    }
                    request_animation_frame(move || {
                        if self.room.get_untracked() == room {
                            platform::animate_reorder(positions);
                            if focus && !platform::focus(&format!("row-{source}")) {
                                platform::focus("room-title");
                            }
                        }
                    });
                }
            }
            Ok(None) => {}
            Err(error) => self.show_toast(error.to_string(), false),
        }
    }
}

#[component]
pub(super) fn Handle(id: TaskId, title: String) -> impl IntoView {
    let c = ctx();
    view! {
        <span class="task-drag-grip" aria-hidden="true" draggable="false"
            title=move || if c.room.with(|room| matches!(room, RoomId::List(_))) { "Drag to reorder or move" } else { "Drag to a list or My Day" }
            on:pointerdown=move |ev| c.start_drag(id, title.clone(), &ev)
            on:pointermove=move |ev| c.move_drag(&ev)
            on:pointerup=move |ev| c.end_drag(&ev)
            on:pointercancel=move |_| { c.cancel_task_drag(); }
            on:lostpointercapture=move |_| { c.cancel_task_drag(); }
            on:dragstart=move |ev| ev.prevent_default()
            on:click=move |ev| ev.stop_propagation()>
            {icon(Icon::GripVertical)}
        </span>
    }
}

#[component]
pub(super) fn Lift() -> impl IntoView {
    let c = ctx();
    let node = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if let Some(el) = node.get()
            && let Some(g) = c.task_drag.get().filter(|g| g.active)
        {
            let style = HtmlElement::style(&el);
            let _ = style.set_property("--task-drag-x", &format!("{}px", g.x));
            let _ = style.set_property("--task-drag-y", &format!("{}px", g.y));
        }
    });
    view! {
        <div class="task-drag-lift" node_ref=node aria-hidden="true" hidden=move || !c.drag_active()
            class:task-drag-touch=move || c.task_drag.with(|g| g.as_ref().is_some_and(|g| g.touch))>
            {icon(Icon::GripVertical)}
            <span class="task-drag-title">{move || c.task_drag.with(|g| g.as_ref().map(|g| g.title.clone()))}</span>
            {move || c.task_drag.with(|g| g.as_ref().map_or(0, |g| g.drag.ids().len())).checked_sub(1).filter(|&n| n > 0).map(|n| view! { <span class="task-drag-count">{n+1}</span> })}
        </div>
    }
}

pub(super) fn destination(room: &RoomId) -> Option<Destination> {
    match room {
        RoomId::Smart("my_day") => Some(Destination::MyDay),
        RoomId::List(list) => Some(Destination::List(*list)),
        _ => None,
    }
}

pub(super) fn destination_attr(dest: Destination) -> String {
    match dest {
        Destination::MyDay => "my-day".into(),
        Destination::List(id) => id.to_string(),
    }
}
