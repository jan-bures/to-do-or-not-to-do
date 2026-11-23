//! Sidebar room drags use the same capture and edge scrolling as task drags.

use std::time::Duration;

use gpui::{
    Bounds, Context, DispatchPhase, Div, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Stateful, Window, canvas, div,
    point, prelude::*, px,
};
use todo_client::sidebar_drag::{Drag, Row, Target};
use todo_core::Command;
use todo_design::motion;

use super::{RoomKey, ShellView, kit, task_drag::edge_scroll};
use crate::theme::{Glyph, Hue, TextStyled, desktop, elevation, list_colors, radius, space, ty};

#[derive(Clone)]
pub(super) struct Badge {
    pub title: SharedString,
    pub glyph: Glyph,
    pub hue: Hue,
    pub count: u32,
}

pub(super) struct Gesture {
    plan: Drag,
    badge: Badge,
    start: Point<Pixels>,
    position: Point<Pixels>,
    active: bool,
    target: Option<Target>,
    dwell: Duration,
    fixture: bool,
}

fn room(row: Row) -> RoomKey {
    match row {
        Row::List(id) => RoomKey::List(id),
        Row::Collection(id) => RoomKey::Collection(id),
    }
}

fn row(key: RoomKey) -> Option<Row> {
    match key {
        RoomKey::List(id) => Some(Row::List(id)),
        RoomKey::Collection(id) => Some(Row::Collection(id)),
        _ => None,
    }
}

impl ShellView {
    pub(super) fn sidebar_reorder_input(
        &self,
        el: Stateful<Div>,
        row: Row,
        badge: Badge,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(focus) = self.sidebar_row_foci.get(&room(row)) else {
            return el;
        };
        let accent = self.palette.accent_strong;
        el.track_focus(focus)
            .focus_visible(move |style| style.border_color(accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    if window.default_prevented() {
                        return;
                    }
                    let Some(plan) = Drag::start(&this.store.view(), row) else {
                        return;
                    };
                    this.sidebar_eat_release = false;
                    if let Some(focus) = this.sidebar_row_foci.get(&room(row)) {
                        window.focus(focus, cx);
                    }
                    this.sidebar_drag = Some(Gesture {
                        plan,
                        badge: badge.clone(),
                        start: e.position,
                        position: e.position,
                        active: false,
                        target: None,
                        dwell: Duration::ZERO,
                        fixture: false,
                    });
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                let mods = e.keystroke.modifiers;
                match e.keystroke.key.as_str() {
                    "escape" if this.cancel_sidebar_drag(cx) => cx.stop_propagation(),
                    key @ ("up" | "down")
                        if mods.alt
                            && !mods.control
                            && !mods.shift
                            && !mods.platform
                            && this.sidebar_drag.is_none() =>
                    {
                        this.nudge_sidebar_row(row, if key == "down" { 1 } else { -1 }, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
    }

    pub(super) fn nudge_sidebar_row(&mut self, row: Row, by: i32, cx: &mut Context<Self>) {
        if let Some(cmd) = todo_client::intent::nudge(&self.store.view(), row.entity(), by) {
            self.submit(vec![cmd]);
            self.refresh(cx);
        }
    }

    pub(super) fn cancel_sidebar_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.sidebar_drag.take() else {
            return false;
        };
        self.sidebar_eat_release = g.active && !g.fixture;
        cx.notify();
        true
    }

    fn sidebar_hit(&self, g: &Gesture) -> Option<Target> {
        if !self.sidebar_scroll.bounds().contains(&g.position) {
            return None;
        }
        let bounds = self.nav_bounds.borrow();
        let state = self.store.view();
        for (&key, b) in bounds.iter() {
            if !b.contains(&g.position) {
                continue;
            }
            let r = row(key)?;
            if let Row::List(id) = r
                && let Some(parent) = state.lists.get(&id).and_then(|l| l.collection)
            {
                // The row probes are full height, even while clipped by an expanding collection.
                let group = self.sidebar_group_bounds.borrow();
                if !group.get(&parent).is_some_and(|b| b.contains(&g.position)) {
                    continue;
                }
            }
            return g.plan.hit(&state, r, g.position.y >= b.center().y);
        }
        // Margins between two groups are collection slots, not dead zones.
        let groups = self.sidebar_group_bounds.borrow();
        for pair in self.shell.sidebar.collections.windows(2) {
            let [prev, next] = pair else { continue };
            let (Some(prev), Some(head)) = (
                groups.get(&prev.id),
                bounds.get(&RoomKey::Collection(next.id)),
            ) else {
                continue;
            };
            if g.position.y >= prev.bottom()
                && g.position.y < head.top()
                && g.position.x >= head.left()
                && g.position.x < head.right()
            {
                let target = Target::Before(Row::Collection(next.id));
                return g.plan.command(&state, target).map(|_| target);
            }
        }
        None
    }

    pub(super) fn drive_sidebar_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.sidebar_drag.as_ref() else {
            return;
        };
        if g.fixture {
            return;
        }
        if !window.is_window_active() || !g.plan.current(&self.store.view()) {
            self.cancel_sidebar_drag(cx);
            return;
        }
        if !g.active {
            return;
        }
        let target = self.sidebar_hit(g);
        edge_scroll(&self.sidebar_scroll, g.position, window);
        let Some(g) = self.sidebar_drag.as_mut() else {
            return;
        };
        if g.target != target {
            g.target = target;
            g.dwell = self.frame;
        }
        if let Some(Target::Into(id)) = target
            && self.ui.collapsed_collections.contains(&id)
        {
            if self.frame.saturating_sub(g.dwell)
                >= Duration::from_millis(u64::from(motion::SIDEBAR_EXPAND_DWELL))
            {
                self.toggle_collection(id, cx);
            } else {
                window.request_animation_frame();
            }
        }
    }

    pub(super) fn sidebar_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                            let Some(g) = this.sidebar_drag.as_mut().filter(|g| !g.fixture) else {
                                return;
                            };
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.cancel_sidebar_drag(cx);
                                return;
                            }
                            g.position = e.position;
                            let d = (g.position - g.start).map(f32::from);
                            g.active |= d.x.abs().max(d.y.abs()) >= space::S1;
                            if g.active {
                                cx.notify();
                                cx.stop_propagation();
                            }
                        })
                        .ok();
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        if std::mem::take(&mut this.sidebar_eat_release) {
                            cx.stop_propagation();
                        }
                        let Some(mut g) =
                            this.sidebar_drag.take().filter(|g| !g.fixture && g.active)
                        else {
                            return;
                        };
                        cx.stop_propagation();
                        g.position = e.position;
                        if let Some(target) = this.sidebar_hit(&g)
                            && let Some(cmd) = g.plan.command(&this.store.view(), target)
                        {
                            let destination = match cmd {
                                Command::MoveList { collection, .. } => collection,
                                _ => None,
                            };
                            let expected = cmd.clone();
                            if this.submit_if(cmd, |s| g.plan.command(s, target) == Some(expected))
                                == Ok(true)
                            {
                                let expand = destination
                                    .is_some_and(|id| this.ui.collapsed_collections.remove(&id));
                                this.refresh_with_collapse_change(expand, cx);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                });
            },
        )
        .absolute()
        .size_0()
    }

    pub(super) fn sidebar_lift(&self, window: &Window) -> Option<Div> {
        let g = self.sidebar_drag.as_ref().filter(|g| g.active)?;
        let p = self.palette;
        let size = window.viewport_size();
        let w = px(desktop::SIDEBAR);
        let x = (g.position.x + px(space::S3))
            .max(self.sidebar_scroll.bounds().right() + px(space::S2))
            .min(size.width - w - px(space::S2));
        let h = px(desktop::NAV_ROW);
        let y = (g.position.y - h / 2.).clamp(px(space::S2), size.height - h - px(space::S2));
        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(w)
                .h(h)
                .flex()
                .items_center()
                .gap(px(desktop::NAV_GAP))
                .px(px(space::S2))
                .bg(p.pane)
                .rounded(px(radius::BUTTON))
                .border(px(elevation::HAIRLINE))
                .border_color(if self.pace().reduced {
                    p.accent
                } else {
                    p.line_strong
                })
                .shadow(kit::overlay_shadow(&p))
                .child(kit::glyph(
                    g.badge.glyph,
                    list_colors(self.theme, g.badge.hue).glyph,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .style(ty::NAV_SUB)
                        .font_weight(FontWeight::MEDIUM)
                        .child(g.badge.title.clone()),
                )
                .child(
                    div()
                        .style(ty::COUNT)
                        .text_color(p.ink_3)
                        .child(g.badge.count.to_string()),
                ),
        )
    }

    pub(super) fn sidebar_target(&self, target: Target) -> bool {
        self.sidebar_drag
            .as_ref()
            .is_some_and(|g| g.target == Some(target))
    }

    pub(super) fn sidebar_source(&self, key: RoomKey) -> bool {
        self.sidebar_drag
            .as_ref()
            .is_some_and(|g| g.active && room(g.plan.source) == key)
    }

    pub(super) fn sidebar_line(&self, after: bool, indent: f32) -> Div {
        div()
            .absolute()
            .when(after, |d| d.bottom_0())
            .when(!after, |d| d.top_0())
            .left(px(indent))
            .right(px(space::S2))
            .h(px(elevation::SELECTION_BAR))
            .rounded(px(radius::BUTTON))
            .bg(self.palette.accent_strong)
    }

    pub(super) fn preview_sidebar_drag(&mut self, fixture: &str, cx: &mut Context<Self>) {
        let [first, second, ..] = self.shell.sidebar.collections.as_slice() else {
            return;
        };
        let [a, _, c, ..] = first.lists.as_slice() else {
            return;
        };
        let (source, badge, visual, target) = if fixture == "sidebar-drag-collection" {
            (
                Row::Collection(first.id),
                Badge {
                    title: first.name.clone(),
                    glyph: first.glyph,
                    hue: first.hue,
                    count: first.count,
                },
                RoomKey::Collection(second.id),
                Target::After(Row::Collection(second.id)),
            )
        } else {
            let (visual, target) = if fixture == "sidebar-drag-target" {
                (RoomKey::Collection(second.id), Target::Into(second.id))
            } else {
                (RoomKey::List(c.id), Target::Before(Row::List(c.id)))
            };
            (
                Row::List(a.id),
                Badge {
                    title: a.list.name.clone(),
                    glyph: a.glyph,
                    hue: a.list.hue,
                    count: a.count,
                },
                visual,
                target,
            )
        };
        let Some(plan) = Drag::start(&self.store.view(), source) else {
            return;
        };
        let position = self
            .nav_bounds
            .borrow()
            .get(&visual)
            .map(Bounds::center)
            .unwrap_or(point(
                px(space::S10),
                px(desktop::TITLEBAR + desktop::NAV_ROW * 11.),
            ));
        self.sidebar_drag = Some(Gesture {
            plan,
            badge,
            start: position,
            position,
            active: true,
            target: Some(target),
            dwell: self.frame,
            fixture: true,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, store::Store, theme::Theme};
    use std::rc::Rc;

    #[gpui::test]
    fn pointer_drop_moves_one_list_and_sidebar_shortcut_is_undoable(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let fixture = fixtures::by_name("sidebar-drag-list").expect("fixture");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                Rc::new(Store::fixture(&fixture)),
                fixture.ui,
                Theme::Light,
                None,
                window,
                cx,
            );
            view.still();
            window.activate_window();
            view
        });
        let mods = gpui::Modifiers::default();
        cx.simulate_mouse_move(point(px(0.), px(0.)), None, mods);
        let (list, source, target, collection, initial_room) = cx.update(|_, cx| {
            let v = shell.read(cx);
            let list = v.shell.sidebar.collections[0].lists[0].id;
            let collection = v.shell.sidebar.collections[1].id;
            let bounds = v.nav_bounds.borrow();
            (
                list,
                bounds[&RoomKey::List(list)].center(),
                bounds[&RoomKey::Collection(collection)].center(),
                collection,
                v.shell.room.key,
            )
        });
        cx.simulate_mouse_move(source, None, mods);
        cx.simulate_mouse_down(source, MouseButton::Left, mods);
        cx.simulate_mouse_move(target, MouseButton::Left, mods);
        cx.update(|_, cx| assert!(shell.read(cx).sidebar_target(Target::Into(collection))));
        cx.simulate_keystrokes("escape");
        cx.simulate_mouse_up(target, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert!(v.sidebar_drag.is_none());
            assert_ne!(v.store.view().lists[&list].collection, Some(collection));
        });
        cx.simulate_mouse_move(source, None, mods);
        cx.simulate_mouse_down(source, MouseButton::Left, mods);
        cx.simulate_mouse_move(target, MouseButton::Left, mods);
        cx.simulate_mouse_up(target, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(v.shell.room.key, initial_room);
            assert_eq!(v.store.view().lists[&list].collection, Some(collection));
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert_ne!(
                shell.read(cx).store.view().lists[&list].collection,
                Some(collection)
            )
        });
        cx.simulate_mouse_move(source, None, mods);
        cx.simulate_click(source, mods);
        cx.update(|_, cx| assert_eq!(shell.read(cx).shell.room.key, RoomKey::List(list)));
        cx.simulate_keystrokes("alt-down");
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).shell.sidebar.collections[0].lists[1].id,
                list
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).shell.sidebar.collections[0].lists[0].id,
                list
            )
        });
    }
}
