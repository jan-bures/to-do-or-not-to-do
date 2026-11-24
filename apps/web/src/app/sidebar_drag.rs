//! Pointer events on sidebar links, with capture on the stable scroll container after pickup.

use super::{Ctx, ctx, document, glyph, model, platform};
use leptos::prelude::*;
use todo_client::sidebar_drag::{self, Drag, Row, Target};
use todo_design::{layout::space, motion};
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, PointerEvent};

#[derive(Clone)]
pub(super) struct Gesture {
    plan: Drag,
    item: model::NavItem,
    pointer: i32,
    start: (i32, i32),
    at: (i32, i32),
    grab: (f64, f64),
    size: (f64, f64),
    active: bool,
    target: Option<Target>,
    dwell: f64,
}

pub(super) fn row(room: &model::RoomId) -> Option<Row> {
    match room {
        model::RoomId::List(id) => Some(Row::List(*id)),
        model::RoomId::Collection(id) => Some(Row::Collection(*id)),
        _ => None,
    }
}

pub(super) fn attr(row: Row) -> String {
    match row {
        Row::List(id) => format!("list:{id}"),
        Row::Collection(id) => format!("collection:{id}"),
    }
}

impl Ctx {
    pub(super) fn sidebar_target(self, target: Target) -> bool {
        self.sidebar_drag
            .with(|g| g.as_ref().is_some_and(|g| g.target == Some(target)))
    }

    pub(super) fn sidebar_active(self) -> bool {
        self.sidebar_drag
            .with(|g| g.as_ref().is_some_and(|g| g.active))
    }

    pub(super) fn sidebar_source(self, row: Row) -> bool {
        self.sidebar_drag
            .with(|g| g.as_ref().is_some_and(|g| g.active && g.plan.source == row))
    }

    pub(super) fn cancel_sidebar_drag(self) -> bool {
        let Some(g) = self.sidebar_drag.try_update(Option::take).flatten() else {
            return false;
        };
        self.sidebar_click_blocked.set(g.active);
        if let Some(nav) = document().get_element_by_id("room-navigation") {
            let _ = nav.release_pointer_capture(g.pointer);
        }
        true
    }

    pub(super) fn start_sidebar_drag(self, item: model::NavItem, ev: &PointerEvent) {
        if !ev.is_primary() || ev.button() != 0 {
            return;
        }
        self.sidebar_click_blocked.set(false);
        if self.read_only() || self.sidebar_drag.get_untracked().is_some_and(|g| g.active) {
            return;
        }
        let Some(plan) =
            row(&item.room).and_then(|r| self.state.with_untracked(|s| Drag::start(s, r)))
        else {
            return;
        };
        let Some(link) = ev
            .current_target()
            .and_then(|e| e.dyn_into::<Element>().ok())
        else {
            return;
        };
        let bounds = link.get_bounding_client_rect();
        let at = (ev.client_x(), ev.client_y());
        self.sidebar_drag.set(Some(Gesture {
            plan,
            item,
            pointer: ev.pointer_id(),
            start: at,
            at,
            grab: (f64::from(at.0) - bounds.x(), f64::from(at.1) - bounds.y()),
            size: (bounds.width(), bounds.height()),
            active: false,
            target: None,
            dwell: 0.,
        }));
    }

    pub(super) fn leave_sidebar_drag(self) {
        if !self.sidebar_active() {
            self.sidebar_drag.set(None);
        }
    }

    pub(super) fn move_sidebar_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .sidebar_drag
            .get_untracked()
            .filter(|g| g.pointer == ev.pointer_id())
        else {
            return;
        };
        if ev.buttons() & 1 == 0 {
            self.cancel_sidebar_drag();
            return;
        }
        g.at = (ev.client_x(), ev.client_y());
        let first = !g.active;
        if first {
            if (g.at.0 - g.start.0).abs().max((g.at.1 - g.start.1).abs()) < space::S1 as i32 {
                return;
            }
            let Some(nav) = ev
                .current_target()
                .and_then(|e| e.dyn_into::<Element>().ok())
            else {
                return;
            };
            if nav.set_pointer_capture(g.pointer).is_err() {
                return;
            }
            g.active = true;
        }
        ev.prevent_default();
        self.update_sidebar_hit(&mut g);
        self.sidebar_drag.set(Some(g));
        if first {
            request_animation_frame(move || self.drive_sidebar_drag());
        }
    }

    fn sidebar_hit(self, g: &Gesture) -> Option<Target> {
        let element = document().element_from_point(g.at.0 as f32, g.at.1 as f32)?;
        let y = f64::from(g.at.1);
        let (entry, after) = if let Some(entry) = element.closest("[data-sidebar-row]").ok()? {
            let rect = entry.get_bounding_client_rect();
            (entry, y >= rect.top() + rect.height() * 0.5)
        } else if matches!(g.plan.source, Row::Collection(_)) {
            // A real gap between groups belongs to the following collection's leading slot.
            let groups = element.closest(".nav-collections").ok()??;
            let anchors = groups
                .query_selector_all(".nav-collection[data-sidebar-row]")
                .ok()?;
            let entry = (0..anchors.length())
                .filter_map(|index| anchors.item(index)?.dyn_into::<Element>().ok())
                .find(|head| {
                    let rect = head.get_bounding_client_rect();
                    rect.top() >= y
                        && f64::from(g.at.0) >= rect.left()
                        && f64::from(g.at.0) < rect.right()
                })?;
            (entry, false)
        } else {
            return None;
        };
        let key = entry.get_attribute("data-sidebar-row")?;
        let (kind, id) = key.split_once(':')?;
        let row = match kind {
            "list" => Row::List(id.parse().ok()?),
            "collection" => Row::Collection(id.parse().ok()?),
            _ => return None,
        };
        self.state
            .with_untracked(|state| g.plan.hit(state, row, after))
    }

    fn update_sidebar_hit(self, g: &mut Gesture) {
        let target = self.sidebar_hit(g);
        if target != g.target {
            g.target = target;
            g.dwell = js_sys::Date::now();
        }
    }

    fn drive_sidebar_drag(self) {
        let Some(mut g) = self
            .sidebar_drag
            .try_get_untracked()
            .flatten()
            .filter(|g| g.active)
        else {
            return;
        };
        let Some(nav) = document().get_element_by_id("room-navigation") else {
            self.cancel_sidebar_drag();
            return;
        };
        let rect = nav.get_bounding_client_rect();
        if rect.width() == 0. || !self.state.with_untracked(|s| g.plan.current(s)) {
            self.cancel_sidebar_drag();
            return;
        }
        let (x, y) = (f64::from(g.at.0), f64::from(g.at.1));
        if x >= rect.left() && x < rect.right() && y >= rect.top() && y < rect.bottom() {
            let edge = f64::from(space::S8);
            let step = |distance: f64| {
                ((edge - distance).clamp(0., edge) / edge * f64::from(space::S3)).ceil() as i32
            };
            let delta = step(rect.bottom() - y) - step(y - rect.top());
            if delta != 0 {
                nav.set_scroll_top(nav.scroll_top() + delta);
            }
        }
        let previous = g.target;
        self.update_sidebar_hit(&mut g);
        if g.target != previous {
            self.sidebar_drag.set(Some(g.clone()));
        }
        if let Some(Target::Into(id)) = g.target
            && js_sys::Date::now() - g.dwell >= f64::from(motion::SIDEBAR_EXPAND_DWELL)
            && self.collapsed.with_untracked(|ids| ids.contains(&id))
        {
            self.toggle_fold(id);
        }
        request_animation_frame(move || self.drive_sidebar_drag());
    }

    pub(super) fn end_sidebar_drag(self, ev: &PointerEvent) {
        let Some(mut g) = self
            .sidebar_drag
            .get_untracked()
            .filter(|g| g.pointer == ev.pointer_id())
        else {
            return;
        };
        if !g.active {
            self.sidebar_drag.set(None);
            return;
        }
        g.at = (ev.client_x(), ev.client_y());
        let target = self.sidebar_hit(&g);
        self.cancel_sidebar_drag();
        ev.prevent_default();
        if let Some(cmd) =
            target.and_then(|target| self.state.with_untracked(|s| g.plan.command(s, target)))
        {
            self.apply_sidebar_drop(g.plan.source, cmd, false);
        }
    }

    pub(super) fn nudge_sidebar(self, row: Row, down: bool) {
        if let Some(cmd) = self.state.with_untracked(|s| {
            todo_client::intent::nudge(s, row.entity(), if down { 1 } else { -1 })
        }) {
            self.apply_sidebar_drop(row, cmd, true);
        }
    }

    fn apply_sidebar_drop(self, row: Row, command: todo_core::Command, focus: bool) {
        let label = sidebar_drag::label(&command);
        let destination = match command {
            todo_core::Command::MoveList { collection, .. } => collection,
            _ => None,
        };
        if !self.submit_undoable(command, label.into(), None) {
            return;
        }
        // The drawer is modal on narrow screens; keep the Undo action in that focus scope.
        self.toast.update(|toast| {
            if let Some(toast) = toast {
                toast.sidebar = true;
            }
        });
        if let Some(id) = destination
            && self.collapsed.with_untracked(|ids| ids.contains(&id))
        {
            self.toggle_fold(id);
        }
        if focus {
            let selector = format!("[data-sidebar-row=\"{}\"] > a", attr(row));
            request_animation_frame(move || {
                if let Ok(Some(el)) = document().query_selector(&selector) {
                    platform::focus(&el.id());
                }
            });
        }
    }
}

#[component]
pub(super) fn Lift() -> impl IntoView {
    let c = ctx();
    let node = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if let Some(el) = node.get()
            && let Some(g) = c.sidebar_drag.get().filter(|g| g.active)
        {
            let style = HtmlElement::style(&el);
            for (property, value) in [
                ("--sidebar-drag-x", f64::from(g.at.0) - g.grab.0),
                ("--sidebar-drag-y", f64::from(g.at.1) - g.grab.1),
                ("--sidebar-drag-width", g.size.0),
                ("--sidebar-drag-height", g.size.1),
            ] {
                let _ = style.set_property(property, &format!("{value}px"));
            }
        }
    });
    let item = Memo::new(move |_| c.sidebar_drag.with(|g| g.as_ref().map(|g| g.item.clone())));
    view! {
        <div class="sidebar-drag-lift" node_ref=node aria-hidden="true" hidden=move || !c.sidebar_active()
            data-list-hue=move || item.with(|n| n.as_ref().map(|n| n.hue))>
            {move || item.with(|n| n.as_ref().and_then(|n| n.glyph).map(glyph))}
            <span class="task-drag-title">{move || item.with(|n| n.as_ref().map(|n| n.name.clone()))}</span>
            <span class="count">{move || item.with(|n| n.as_ref().map(|n| n.count.to_string()))}</span>
        </div>
    }
}
