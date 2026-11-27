//! Pointer-captured step drag reordering inside the task detail pane.

use super::{Ctx, ctx, document, icon, platform};
use leptos::prelude::*;
use todo_client::intent;
use todo_core::ids::{StepId, TaskId};
use todo_design::{Icon, layout::space};
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, PointerEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Slot {
    pub row: StepId,
    pub after: bool,
}

#[derive(Clone)]
pub(super) struct Gesture {
    task: TaskId,
    source: StepId,
    title: String,
    pointer: i32,
    touch: bool,
    x: i32,
    y: i32,
    start: (i32, i32),
    active: bool,
    target: Option<Slot>,
}

impl Ctx {
    pub(super) fn step_drag_active(self) -> bool {
        self.step_drag
            .with(|g| g.as_ref().is_some_and(|g| g.active))
    }

    pub(super) fn step_drop_target(self, slot: Slot) -> bool {
        self.step_drag
            .with(|g| g.as_ref().is_some_and(|g| g.target == Some(slot)))
    }

    pub(super) fn step_dragging(self, id: StepId) -> bool {
        self.step_drag
            .with(|g| g.as_ref().is_some_and(|g| g.active && g.source == id))
    }

    pub(super) fn cancel_step_drag(self) -> bool {
        let Some(g) = self.step_drag.try_update(Option::take).flatten() else {
            return false;
        };
        g.active
    }

    fn take_step_target(self, g: &Gesture) -> Option<Slot> {
        let el = document().element_from_point(g.x as f32, g.y as f32)?;
        let row = el.closest(".step-rows > [data-step-row]").ok().flatten()?;
        let rect = row.get_bounding_client_rect();
        let slot = Slot {
            row: row.get_attribute("data-step-row")?.parse().ok()?,
            after: f64::from(g.y) >= rect.top() + rect.height() * 0.5,
        };
        self.state
            .with_untracked(|s| intent::reorder_step(s, g.source, slot.row, slot.after))
            .map(|_| slot)
    }

    fn start_step_drag(self, task: TaskId, id: StepId, title: String, ev: &PointerEvent) {
        if ev.button() != 0 || !ev.is_primary() || self.read_only() {
            return;
        }
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
        self.step_drag.set(Some(Gesture {
            task,
            source: id,
            title,
            pointer: ev.pointer_id(),
            touch: ev.pointer_type() == "touch",
            x: ev.client_x(),
            y: ev.client_y(),
            start: (ev.client_x(), ev.client_y()),
            active: false,
            target: None,
        }));
    }

    fn move_step_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .step_drag
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
        g.target = self.take_step_target(&g);
        self.step_drag.set(Some(g));
        if first {
            request_animation_frame(move || self.scroll_step_drag());
        }
    }

    fn scroll_step_drag(self) {
        let Some(mut g) = self
            .step_drag
            .try_get_untracked()
            .flatten()
            .filter(|g| g.active)
        else {
            return;
        };
        let el = document().element_from_point(g.x as f32, g.y as f32);
        if let Some(scroll) = el.and_then(|e| e.closest(".detail-scroll").ok().flatten()) {
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
        let target = self.take_step_target(&g);
        if g.target != target {
            g.target = target;
            self.step_drag.set(Some(g));
        }
        request_animation_frame(move || self.scroll_step_drag());
    }

    fn end_step_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .step_drag
            .get_untracked()
            .filter(|g| g.pointer == ev.pointer_id())
        else {
            return;
        };
        g.x = ev.client_x();
        g.y = ev.client_y();
        let target = self.take_step_target(&g);
        self.cancel_step_drag();
        let current_task = self.detail.with_untracked(|d| d.as_ref().map(|d| d.id));
        if !g.active || self.read_only() || current_task != Some(g.task) {
            return;
        }
        let Some(slot) = target else { return };
        if let Some(cmd) = self
            .state
            .with_untracked(|s| intent::reorder_step(s, g.source, slot.row, slot.after))
        {
            self.submit_undoable(cmd, "Step moved.".into(), None);
        }
    }

    pub(super) fn nudge_step(self, id: StepId, by: i32) {
        if self.read_only() {
            return;
        }
        if let Some(cmd) = self
            .state
            .with_untracked(|s| intent::nudge(s, id.into(), by))
            && self.submit_undoable(cmd, "Step moved.".into(), None)
        {
            request_animation_frame(move || platform::refocus_row(&format!("step-title-{id}")));
        }
    }
}

#[component]
pub(super) fn Handle(task: TaskId, id: StepId, title: Signal<String>) -> impl IntoView {
    let c = ctx();
    view! {
        <span class="step-drag-grip" aria-hidden="true" draggable="false" title="Drag to reorder"
            on:pointerdown=move |ev| c.start_step_drag(task, id, title.get_untracked(), &ev)
            on:pointermove=move |ev| c.move_step_drag(&ev)
            on:pointerup=move |ev| c.end_step_drag(&ev)
            on:pointercancel=move |_| { c.cancel_step_drag(); }
            on:lostpointercapture=move |_| { c.cancel_step_drag(); }
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
            && let Some(g) = c.step_drag.get().filter(|g| g.active)
        {
            let style = HtmlElement::style(&el);
            let _ = style.set_property("--task-drag-x", &format!("{}px", g.x));
            let _ = style.set_property("--task-drag-y", &format!("{}px", g.y));
        }
    });
    view! {
        <div class="step-drag-lift" node_ref=node aria-hidden="true" hidden=move || !c.step_drag_active()
            class:task-drag-touch=move || c.step_drag.with(|g| g.as_ref().is_some_and(|g| g.touch))>
            {icon(Icon::GripVertical)}
            <span class="task-drag-title">{move || c.step_drag.with(|g| g.as_ref().map(|g| g.title.clone()))}</span>
        </div>
    }
}
