//! Internal pointer capture for step drag reordering inside the task detail pane.

use super::{ShellView, kit, task_drag::edge_scroll};
use crate::model::commands;
use crate::theme::{Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use gpui::{
    Context, CursorStyle, DispatchPhase, Div, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, SharedString, Window, canvas, div, prelude::*, px,
};
use todo_core::ids::{StepId, TaskId};
use todo_design::motion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    row: StepId,
    after: bool,
}

#[derive(Debug)]
pub(super) struct Gesture {
    task: TaskId,
    source: StepId,
    title: SharedString,
    start: Point<Pixels>,
    position: Point<Pixels>,
    lifted: Option<std::time::Duration>,
    target: Option<Slot>,
}

impl ShellView {
    pub(super) fn step_drag_handle(
        &self,
        id: StepId,
        task: TaskId,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = self.palette;
        div()
            .id(SharedString::from(format!("step-grip-{id}")))
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(space::S2))
            .w(px(space::S4))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::OpenHand)
            .tooltip(kit::tooltip(p, "Drag to reorder"))
            .child(kit::icon(
                Icon::GripVertical,
                component::ICON_SMALL,
                p.ink_3,
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _window, cx| {
                    if this.commit_edits(cx) {
                        this.step_drag = Some(Gesture {
                            task,
                            source: id,
                            title: title.clone(),
                            start: e.position,
                            position: e.position,
                            lifted: None,
                            target: None,
                        });
                        cx.stop_propagation();
                    }
                }),
            )
    }

    pub(super) fn step_bounds_probe(&self, id: StepId) -> impl IntoElement {
        let bounds = self.step_bounds.clone();
        canvas(
            move |b, _, _| {
                bounds.borrow_mut().insert(id, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn cancel_step_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.step_drag.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn step_drag_active(&self) -> bool {
        self.step_drag.as_ref().is_some_and(|g| g.lifted.is_some())
    }

    pub(super) fn step_lifted(&self, id: StepId) -> bool {
        self.step_drag
            .as_ref()
            .is_some_and(|g| g.lifted.is_some() && g.source == id)
    }

    pub(super) fn step_drag_slot(&self, id: StepId) -> Option<bool> {
        let slot = self.step_drag.as_ref()?.target?;
        (slot.row == id).then_some(slot.after)
    }

    fn step_target(&self, g: &Gesture) -> Option<Slot> {
        let detail = self.shell.detail.as_ref()?;
        if detail.task != g.task || !self.detail_scroll.bounds().contains(&g.position) {
            return None;
        }
        let bounds = self.step_bounds.borrow();
        let slot = detail.steps.iter().find_map(|step| {
            let b = bounds.get(&step.id)?;
            b.contains(&g.position).then_some(Slot {
                row: step.id,
                after: g.position.y >= b.center().y,
            })
        })?;
        let state = self.store.view();
        commands::reorder_step(&state, g.source, slot.row, slot.after).map(|_| slot)
    }

    pub(super) fn drive_step_drag(&mut self, window: &mut Window) {
        let Some(g) = &self.step_drag else { return };
        if g.lifted.is_none() {
            return;
        }
        let live =
            self.shell.detail.as_ref().is_some_and(|d| {
                d.task == g.task && d.steps.iter().any(|step| step.id == g.source)
            });
        if !live || !window.is_window_active() {
            self.step_drag = None;
            return;
        }
        let target = self.step_target(g);
        edge_scroll(&self.detail_scroll, g.position, window);
        if let Some(g) = &mut self.step_drag {
            g.target = target;
        }
    }

    pub(super) fn step_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                            let Some(g) = &mut this.step_drag else { return };
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.cancel_step_drag(cx);
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
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        let Some(mut g) = this.step_drag.take() else {
                            return;
                        };
                        g.position = e.position;
                        if g.lifted.is_some()
                            && let Some(slot) = this.step_target(&g)
                        {
                            let cmd = commands::reorder_step(
                                &this.store.view(),
                                g.source,
                                slot.row,
                                slot.after,
                            );
                            this.step_command(cmd, cx);
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

    pub(super) fn step_drag_preview(&self, window: &mut Window) -> Option<Div> {
        let g = self.step_drag.as_ref()?;
        let at = g.lifted?;
        let p = self.palette;
        let width = px(desktop::DETAIL - 2. * desktop::DETAIL_PAD_X)
            .min(window.viewport_size().width - px(2. * space::S2));
        let x = (g.position.x + px(space::S3))
            .max(px(space::S2))
            .min(window.viewport_size().width - width - px(space::S2));
        let y = (g.position.y - px(desktop::STEP_ROW / 2.))
            .max(px(space::S2))
            .min(window.viewport_size().height - px(desktop::STEP_ROW + space::S2));
        let elapsed = self.frame.saturating_sub(at);
        let span = std::time::Duration::from_millis(motion::HOVER.into());
        if elapsed < span {
            window.request_animation_frame();
        }
        let alpha = motion::STANDARD.ease((elapsed.as_secs_f32() / span.as_secs_f32()).min(1.));
        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(width)
                .h(px(desktop::STEP_ROW))
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
                .shadow(kit::overlay_shadow(&p))
                .cursor(CursorStyle::ClosedHand)
                .child(kit::icon(
                    Icon::GripVertical,
                    component::ICON_SMALL,
                    p.ink_3,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .style(ty::STEP)
                        .text_color(p.ink)
                        .child(g.title.clone()),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    #[gpui::test]
    fn pointer_grip_reorders_a_step_and_undo_restores_it(cx: &mut gpui::TestAppContext) {
        use crate::{fixtures, store::Store, theme::Theme};
        use std::rc::Rc;

        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let fixture = fixtures::my_day();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = Rc::new(Store::fixture(&fixture));
            let mut view = ShellView::new(store, fixture.ui, Theme::Light, None, window, cx);
            view.still();
            window.activate_window();
            view
        });
        cx.simulate_mouse_move(point(px(0.), px(0.)), None, gpui::Modifiers::default());
        let (before, grip, target) = cx.update(|_, cx| {
            let v = shell.read(cx);
            let steps: Vec<StepId> = v
                .shell
                .detail
                .as_ref()
                .expect("detail")
                .steps
                .iter()
                .map(|s| s.id)
                .collect();
            assert!(steps.len() >= 3);
            let bounds = v.step_bounds.borrow();
            let first = bounds[&steps[0]];
            let third = bounds[&steps[2]];
            let grip = point(first.left() + px(space::S4), first.center().y);
            let target = point(third.center().x, third.bottom() - px(2.));
            (steps, grip, target)
        });
        let mods = gpui::Modifiers::default();
        cx.simulate_mouse_move(grip, None, mods);
        cx.simulate_mouse_down(grip, MouseButton::Left, mods);
        cx.update(|_, cx| {
            assert!(
                shell.read(cx).step_drag.is_some(),
                "mouse down should start step drag"
            );
        });
        cx.simulate_mouse_move(target, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let v = shell.read(cx);
            assert_eq!(
                v.step_drag_slot(before[2]),
                Some(true),
                "step_drag={:?}, detail_scroll={:?}, third={:?}",
                v.step_drag,
                v.detail_scroll.bounds(),
                v.step_bounds.borrow().get(&before[2])
            );
        });
        cx.simulate_mouse_up(target, MouseButton::Left, mods);
        cx.update(|_, cx| {
            let after: Vec<StepId> = shell
                .read(cx)
                .shell
                .detail
                .as_ref()
                .expect("detail")
                .steps
                .iter()
                .map(|s| s.id)
                .collect();
            assert_eq!(after[0], before[1]);
            assert_eq!(after[1], before[2]);
            assert_eq!(after[2], before[0]);
        });
        cx.update(|window, cx| {
            let focus = shell.read(cx).list_focus.clone();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            let restored: Vec<StepId> = shell
                .read(cx)
                .shell
                .detail
                .as_ref()
                .expect("detail")
                .steps
                .iter()
                .map(|s| s.id)
                .collect();
            assert_eq!(restored, before);
        });
    }
}
