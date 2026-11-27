//! Internal pointer capture for task drags. GPUI ExternalPaths retains its separate drop path.

use super::{Notice, RoomKey, ShellView, undo};
use crate::theme::{Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, Div, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, ScrollStrategy, SharedString, Window, canvas, div,
    point, prelude::*, px,
};
use todo_client::task_drag::{Destination, Drag, Slot};
use todo_core::ids::{ListId, TaskId};
use todo_core::model::TaskSort;
use todo_design::motion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Nav(RoomKey, Destination),
    Row(Slot),
}

#[derive(Debug)]
pub(super) struct Gesture {
    plan: Drag,
    room: RoomKey,
    title: SharedString,
    start: Point<Pixels>,
    position: Point<Pixels>,
    lifted: Option<std::time::Duration>,
    target: Option<Target>,
    fixture: bool,
}

#[derive(Debug)]
pub(super) struct Settle {
    room: RoomKey,
    tasks: Vec<TaskId>,
    start: std::time::Duration,
}

fn destination(room: RoomKey) -> Option<Destination> {
    match room {
        RoomKey::List(list) => Some(Destination::List(list)),
        room if room == RoomKey::default() => Some(Destination::MyDay),
        _ => None,
    }
}

impl ShellView {
    pub(super) fn drag_handle(
        &self,
        id: TaskId,
        room: RoomKey,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = self.palette;
        let tip = if self.ui.compact {
            "Drag to reorder"
        } else if self.manual_list().is_some() {
            "Drag to reorder or move"
        } else {
            "Drag to a list or My Day"
        };
        div()
            .id(SharedString::from(format!("task-grip-{id}")))
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(space::S2))
            .w(px(space::S4))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::OpenHand)
            .tooltip(super::kit::tooltip(p, tip))
            .child(super::kit::icon(
                Icon::GripVertical,
                component::ICON_SMALL,
                p.ink_3,
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    if this.row_can_act(room, id)
                        && this.commit_edits(cx)
                        && let Some(plan) = Drag::start(&this.store.view(), id, &this.bulk_ids())
                    {
                        window.focus(&this.list_focus, cx);
                        this.task_drag = Some(Gesture {
                            plan,
                            room,
                            title: title.clone(),
                            start: e.position,
                            position: e.position,
                            lifted: None,
                            target: None,
                            fixture: false,
                        });
                        cx.stop_propagation();
                    }
                }),
            )
    }

    pub(super) fn cancel_task_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.task_drag.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn manual_list(&self) -> Option<ListId> {
        match self.shell.room.key {
            RoomKey::List(id)
                if self
                    .store
                    .view()
                    .lists
                    .get(&id)
                    .is_some_and(|list| list.task_sort() == TaskSort::Manual) =>
            {
                Some(id)
            }
            _ => None,
        }
    }

    fn open_order(&self) -> Vec<TaskId> {
        self.shell
            .room
            .tasks
            .iter()
            .filter(|row| self.row_can_act(self.shell.room.key, row.id))
            .map(|row| row.id)
            .collect()
    }

    fn task_target(&self, g: &Gesture) -> Option<Target> {
        if g.room != self.shell.room.key {
            return None;
        }
        let state = self.store.view();
        if self.compact.is_none() && self.sidebar_scroll.bounds().contains(&g.position) {
            let nav = self.nav_bounds.borrow();
            let (&key, _) =
                nav.iter().find(|(key, bounds)| {
                    bounds.contains(&g.position)
                        && match key {
                            RoomKey::List(id) => {
                                self.shell.sidebar.lists.iter().any(|l| l.id == *id)
                                    || self.shell.sidebar.collections.iter().any(|c| {
                                        !c.collapsed && c.lists.iter().any(|l| l.id == *id)
                                    })
                            }
                            room => **room == RoomKey::default(),
                        }
                })?;
            let dest = destination(key)?;
            return g
                .plan
                .eligible(&state, dest, self.store.now().today())
                .then_some(Target::Nav(key, dest));
        }
        let list = self.manual_list()?;
        if !self
            .rows
            .0
            .borrow()
            .base_handle
            .bounds()
            .contains(&g.position)
        {
            return None;
        }
        let open = self.open_order();
        let bounds = self.row_bounds.borrow();
        let slot = open.iter().find_map(|&row| {
            let bounds = bounds.get(&row)?;
            bounds.contains(&g.position).then_some(Slot {
                row,
                after: g.position.y >= bounds.center().y,
            })
        })?;
        g.plan
            .placement(&state, Some(list), &open, slot)
            .map(|_| Target::Row(slot))
    }

    pub(super) fn drive_task_drag(&mut self, window: &mut Window) {
        if let Some(settle) = &self.task_settle {
            if settle.room == self.shell.room.key
                && self.frame.saturating_sub(settle.start)
                    < std::time::Duration::from_millis(motion::HOVER.into())
            {
                window.request_animation_frame();
            } else {
                self.task_settle = None;
            }
        }
        let Some(g) = &self.task_drag else { return };
        if g.fixture || g.lifted.is_none() {
            return;
        }
        if g.room != self.shell.room.key
            || !window.is_window_active()
            || !g.plan.current(&self.store.view())
        {
            self.task_drag = None;
            return;
        }
        let target = self.task_target(g);
        if self.compact.is_none() && self.sidebar_scroll.bounds().contains(&g.position) {
            edge_scroll(&self.sidebar_scroll, g.position, window);
        } else {
            edge_scroll(&self.rows.0.borrow().base_handle, g.position, window);
        }
        if let Some(g) = &mut self.task_drag {
            g.target = target;
        }
    }

    pub(super) fn drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                            let Some(g) = &mut this.task_drag else { return };
                            if g.fixture {
                                return;
                            }
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.cancel_task_drag(cx);
                                return;
                            }
                            g.position = e.position;
                            let distance = (g.position - g.start).map(f32::from);
                            if g.lifted.is_none()
                                && distance.x.abs().max(distance.y.abs()) >= space::S1
                            {
                                g.lifted = Some(this.frame);
                            }
                            if g.lifted.is_some() {
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
                        let Some(mut g) = this.task_drag.take() else {
                            return;
                        };
                        g.position = e.position;
                        if g.lifted.is_some()
                            && !g.fixture
                            && let Some(target) = this.task_target(&g)
                        {
                            let open = this.open_order();
                            let list = this.manual_list();
                            let today = this.store.now().today();
                            let plan = |state: &todo_core::State| match target {
                                Target::Nav(_, dest) => g.plan.drop_on(state, dest, today),
                                Target::Row(slot) => g.plan.drop_in(state, list, &open, slot),
                            };
                            this.apply_drop(&g.plan, matches!(target, Target::Row(_)), plan, cx);
                            window.focus(&this.list_focus, cx);
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

    fn apply_drop(
        &mut self,
        drag: &Drag,
        reorder: bool,
        plan: impl Fn(
            &todo_core::State,
        )
            -> Result<Option<(todo_core::Command, String)>, todo_client::selection::BulkLimit>,
        cx: &mut Context<Self>,
    ) {
        let (cmd, label) = match plan(&self.store.view()) {
            Ok(Some(change)) => change,
            Ok(None) => return,
            Err(error) => {
                self.notice = Some(Notice::error(error.to_string()));
                cx.notify();
                return;
            }
        };
        let affected = match &cmd {
            todo_core::Command::BulkTasks { tasks, .. } => tasks.clone(),
            _ => vec![drag.source],
        };
        match self.store.submit_undoable_if(cmd.clone(), |state| {
            plan(state)
                .ok()
                .flatten()
                .is_some_and(|(live, _)| live == cmd)
        }) {
            Ok(Some(target)) => {
                self.notice = None;
                self.remember_undo(
                    undo::Action {
                        entity: affected.first().copied().unwrap_or(drag.source).into(),
                        kind: undo::EditKind::Bulk,
                        targets: vec![target],
                        room: self.shell.room.key,
                    },
                    label.into(),
                );
                if reorder && self.animate {
                    self.task_settle = Some(Settle {
                        room: self.shell.room.key,
                        tasks: affected,
                        start: self.now(),
                    });
                } else if !reorder {
                    self.multi.clear();
                }
                self.refresh(cx);
                if reorder
                    && let Some(i) = (0..self.shell.row_count())
                        .find(|&i| self.shell.row(i).is_some_and(|r| r.id == drag.source))
                {
                    self.rows.scroll_to_item(i, ScrollStrategy::Nearest);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.notice = Some(Notice::error(error));
                cx.notify();
            }
        }
    }

    pub(super) fn nudge_task_drag(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.task_drag.is_some() {
            return;
        }
        let Some(list) = self.manual_list() else {
            return;
        };
        let selected = self.bulk_ids();
        let Some(source) = self
            .multi
            .cursor()
            .filter(|id| selected.contains(id))
            .or_else(|| selected.first().copied())
            .or(self.ui.task)
        else {
            return;
        };
        let Some(drag) = Drag::start(&self.store.view(), source, &selected) else {
            return;
        };
        let open = self.open_order();
        self.apply_drop(&drag, true, |s| drag.nudge(s, list, &open, down), cx);
    }

    pub(super) fn drop_glow(&self, key: RoomKey) -> bool {
        self.task_drag
            .as_ref()
            .is_some_and(|g| matches!(g.target, Some(Target::Nav(room, _)) if room == key))
    }

    pub(super) fn drag_slot(&self, id: TaskId) -> Option<bool> {
        match self.task_drag.as_ref()?.target? {
            Target::Row(slot) if slot.row == id => Some(slot.after),
            _ => None,
        }
    }

    pub(super) fn row_lifted(&self, id: TaskId) -> bool {
        self.task_drag
            .as_ref()
            .is_some_and(|g| g.lifted.is_some() && g.plan.ids().any(|member| member == id))
    }

    pub(super) fn task_settle_alpha(&self, id: TaskId) -> Option<f32> {
        let settle = self
            .task_settle
            .as_ref()
            .filter(|s| s.tasks.contains(&id))?;
        let elapsed = self.frame.saturating_sub(settle.start);
        Some(
            1. - motion::STANDARD
                .ease((elapsed.as_secs_f32() / (motion::HOVER as f32 / 1000.)).min(1.)),
        )
    }

    pub(super) fn drag_preview(&self, window: &mut Window) -> Option<Div> {
        let g = self.task_drag.as_ref()?;
        let at = g.lifted?;
        let p = self.palette;
        let width = px(desktop::DETAIL).min(window.viewport_size().width - px(2. * space::S2));
        let next_to_sidebar = matches!(g.target, Some(Target::Nav(..)));
        let start_x = if next_to_sidebar {
            self.sidebar_scroll.bounds().right() + px(space::S2)
        } else {
            px(space::S2)
        };
        let x = (g.position.x + px(space::S3))
            .max(start_x)
            .min(window.viewport_size().width - width - px(space::S2));
        let y = (g.position.y - px(component::ICON_BUTTON / 2.))
            .max(px(space::S2))
            .min(window.viewport_size().height - px(component::ICON_BUTTON + space::S2));
        let elapsed = self.frame.saturating_sub(at);
        let span = std::time::Duration::from_millis(motion::HOVER.into());
        if !g.fixture && elapsed < span {
            window.request_animation_frame();
        }
        let alpha = if g.fixture {
            1.
        } else {
            motion::STANDARD.ease((elapsed.as_secs_f32() / span.as_secs_f32()).min(1.))
        };
        let count = g.plan.ids().len();
        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(width)
                .h(px(component::ICON_BUTTON))
                .opacity(alpha)
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .rounded(px(radius::BUTTON))
                .bg(p.pane)
                .border(px(elevation::HAIRLINE))
                .border_color(if self.pace().reduced {
                    p.accent
                } else {
                    p.line_strong
                })
                .shadow(super::kit::overlay_shadow(&p))
                .cursor(CursorStyle::ClosedHand)
                .child(super::kit::icon(
                    Icon::GripVertical,
                    component::ICON_SMALL,
                    p.ink_3,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .style(ty::TASK_TITLE)
                        .child(g.title.clone()),
                )
                .when(count > 1, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .px(px(space::S2))
                            .rounded(px(radius::BUTTON))
                            .bg(p.selection_wash)
                            .style(ty::COUNT)
                            .child(count.to_string()),
                    )
                }),
        )
    }

    pub(super) fn preview_task_drag(
        &mut self,
        fixture: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.open_order();
        let (Some(&source), Some(&last)) = (open.first(), open.last()) else {
            return;
        };
        self.change_selection(
            |s, _, _| {
                s.range(&open, open.get(1).copied().unwrap_or(source), Some(source));
            },
            cx,
        );
        let state = self.store.view();
        let Some(plan) = Drag::start(&state, source, &self.bulk_ids()) else {
            return;
        };
        let Some(row) = self.shell.room.tasks.iter().find(|r| r.id == source) else {
            return;
        };
        let (position, target) = if fixture == "drag-destination" {
            let key = RoomKey::default();
            let pos = self
                .nav_bounds
                .borrow()
                .get(&key)
                .map(Bounds::center)
                .unwrap_or(point(
                    px(space::S10),
                    px(desktop::TITLEBAR + desktop::NAV_ROW),
                ));
            (pos, Target::Nav(key, Destination::MyDay))
        } else {
            let pos = self
                .row_bounds
                .borrow()
                .get(&last)
                .map(|b| point(b.center().x, b.bottom() - px(space::S1)))
                .unwrap_or(point(
                    window.viewport_size().width / 2.,
                    px(desktop::ROW * 7.),
                ));
            (
                pos,
                Target::Row(Slot {
                    row: last,
                    after: true,
                }),
            )
        };
        self.task_drag = Some(Gesture {
            plan,
            title: row.title.clone(),
            room: self.shell.room.key,
            position,
            start: position,
            lifted: Some(self.frame),
            target: Some(target),
            fixture: true,
        });
        cx.notify();
    }
}

pub(super) fn edge_scroll(handle: &ScrollHandle, pointer: Point<Pixels>, window: &mut Window) {
    let view = handle.bounds();
    if !view.contains(&pointer) {
        return;
    }
    let delta = edge_step(
        f32::from(pointer.y - view.top()),
        f32::from(view.bottom() - pointer.y),
    );
    let offset = handle.offset();
    let y = (offset.y - px(delta)).clamp(-handle.max_offset().y, px(0.));
    if y != offset.y {
        handle.set_offset(point(offset.x, y));
        window.request_animation_frame();
    }
}

fn edge_step(top: f32, bottom: f32) -> f32 {
    let strength = |gap: f32| (1. - gap / space::S8).clamp(0., 1.);
    (strength(bottom) - strength(top)) * space::S3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn nudge_moves_a_selected_block_and_undoes_once(cx: &mut gpui::TestAppContext) {
        use crate::{fixtures, store::Store, theme::Theme};
        use std::rc::Rc;

        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let fixture = fixtures::by_name("drag-reorder").expect("fixture");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = Rc::new(Store::fixture(&fixture));
            let mut view = ShellView::new(store, fixture.ui, Theme::Light, None, window, cx);
            view.still();
            window.focus(&view.list_focus, cx);
            view
        });
        let before = cx.update(|_, cx| shell.read(cx).open_order());
        assert_eq!(before.len(), 3);
        cx.simulate_keystrokes("down shift-down alt-down");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.open_order(), [before[2], before[0], before[1]]);
            assert_eq!(view.bulk_ids(), before[..2]);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(shell.read(cx).open_order(), before));
    }

    #[gpui::test]
    fn pointer_grip_moves_to_a_sidebar_list_and_leaves_the_room_open(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{fixtures, store::Store, theme::Theme};
        use std::rc::Rc;

        let fixture = fixtures::by_name("drag-reorder").expect("fixture");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = Rc::new(Store::fixture(&fixture));
            let mut view = ShellView::new(store, fixture.ui, Theme::Light, None, window, cx);
            view.still();
            window.activate_window();
            view
        });
        cx.simulate_mouse_move(point(px(0.), px(0.)), None, gpui::Modifiers::default());
        let (source, room, list, grip, target) = cx.update(|_, cx| {
            let v = shell.read(cx);
            let source = v.open_order()[0];
            let target = v.shell.sidebar.collections[0].lists[1].id;
            let bounds = v.row_bounds.borrow()[&source];
            let grip = point(bounds.left() + px(space::S4), bounds.center().y);
            let center = v.nav_bounds.borrow()[&RoomKey::List(target)].center();
            (source, v.shell.room.key, target, grip, center)
        });
        let mods = gpui::Modifiers::default();
        cx.simulate_mouse_move(grip, None, mods);
        cx.simulate_mouse_down(grip, MouseButton::Left, mods);
        cx.simulate_mouse_move(target, MouseButton::Left, mods);
        cx.update(|_, cx| assert!(shell.read(cx).drop_glow(RoomKey::List(list))));
        cx.simulate_mouse_up(target, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(v.shell.room.key, room);
            assert_eq!(v.store.view().tasks[&source].list, list);
        });
    }

    #[test]
    fn edge_scroll_is_symmetric_and_stops_inside() {
        assert_eq!(edge_step(80., 80.), 0.);
        assert_eq!(edge_step(0., 80.), -space::S3);
        assert_eq!(edge_step(80., 0.), space::S3);
        assert_eq!(edge_step(space::S8 / 2., 80.), -space::S3 / 2.);
    }
}
