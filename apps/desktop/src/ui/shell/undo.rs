//! Undo target ownership and the toast in the list's reserved footer. The target outlives the toast.

use std::sync::Arc;

use gpui::{Context, Div, MouseButton, Role, Window, div, prelude::*, px};
use todo_client::{CommandId, TrackedUndo};
use todo_core::ids::{EntityId, TaskId};
use todo_core::{Command, State};

use super::{Icon, Notice, ShellView, component, desktop, elevation, kit, radius, space, ty};
use crate::model::labels::one_line;
use crate::store::UndoTarget;
use crate::theme::TextStyled;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Complete,
    Delete,
    Move,
}

impl EditKind {
    pub(super) fn of(command: &Command) -> Option<(TaskId, Self)> {
        Some(match *command {
            Command::CompleteTask { id } => (id, Self::Complete),
            Command::Delete {
                id: EntityId::Task(id),
            } => (id, Self::Delete),
            Command::MoveTask { id, .. }
            | Command::Reorder {
                id: EntityId::Task(id),
                ..
            } => (id, Self::Move),
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Complete => "Completed",
            Self::Delete => "Deleted",
            Self::Move => "Moved",
        }
    }
}

#[derive(Debug)]
pub(super) struct Action {
    pub id: TaskId,
    pub kind: EditKind,
    pub target: UndoTarget,
}

#[derive(Debug)]
pub(super) struct Pending {
    action: Action,
    undo_id: CommandId,
    /// Only a held completion needs speculative display to reverse its animation right away.
    preview: bool,
    /// Already started settling the held row toward the core preview.
    animated: bool,
}

impl ShellView {
    pub(super) fn remember_undo(&mut self, action: Action, title: &str) {
        let (now, pace) = (self.now(), self.pace());
        self.toast.advance(now, pace);
        self.toast.show(
            format!(
                "{} \u{201c}{}\u{201d}",
                action.kind.label(),
                one_line(title)
            )
            .into(),
            now,
            pace,
        );
        self.last_undo = Some(action);
    }

    pub(super) fn request_undo(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.last_undo.take() else {
            return;
        };
        let (now, pace) = (self.now(), self.pace());
        match self.store.undo(&action.target) {
            Err(error) => {
                // In particular a cache write failure must leave the action retryable.
                self.notice = Some(Notice::error(format!("Couldn't undo that: {error}")));
                self.last_undo = Some(action);
                cx.notify();
                return;
            }
            Ok(TrackedUndo::Unavailable) => self.undo_unavailable(),
            Ok(TrackedUndo::Cancelled) => {
                self.notice = None;
                self.animate_restored(&action, &self.store.view());
            }
            Ok(TrackedUndo::Queued(undo_id)) => {
                self.notice = None;
                let preview =
                    action.kind == EditKind::Complete && self.motion.completing(action.id);
                self.pending_undo.push(Pending {
                    action,
                    undo_id,
                    preview,
                    animated: false,
                });
            }
        }
        self.toast.dismiss(now, pace);
        self.refresh(cx);
    }

    fn undo_unavailable(&mut self) {
        self.notice = Some(Notice::error("That change is no longer available to undo."));
    }

    fn animate_restored(&mut self, action: &Action, state: &State) {
        if self.animate
            && let Some(task) = state.tasks.get(&action.id)
            && state.is_live(task.id.into())
        {
            self.motion
                .restore(task.id, task.is_done(), self.now(), self.pace());
        }
    }

    /// Animate the compensation only once, and surface refused undos in the status bar.
    /// The optional preview is presentation only; commands always use the real replica.
    pub(super) fn undo_state(&mut self) -> Arc<State> {
        let mut preview = None;
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.pending_undo) {
            if let Some(result) = self.store.undo_result(pending.undo_id) {
                match result {
                    Ok(()) if !pending.animated => {
                        self.animate_restored(&pending.action, &self.store.view())
                    }
                    Ok(()) => {}
                    Err(e) => self.notice = Some(Notice::error(format!("Couldn't undo that: {e}"))),
                }
                continue;
            }
            if pending.preview
                && let Some(state) = self.store.undo_preview(&pending.action.target)
            {
                if !pending.animated {
                    self.animate_restored(&pending.action, &state);
                    pending.animated = true;
                }
                preview = Some(state);
            }
            waiting.push(pending);
        }
        self.pending_undo = waiting;
        preview.unwrap_or_else(|| self.store.view())
    }

    /// This footer is outside the scroll viewport and the narrow detail overlay. The overlay
    /// keeps Completed inert, but the same Undo is always visible and reachable.
    pub(super) fn list_footer(
        &self,
        completed_enabled: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let open = self.ui.show_completed;
        let count = self.shell.room.completed_count;
        let completed = (count > 0).then(|| {
            div()
                .id("completed-toggle")
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(kit::icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    component::ICON_SMALL,
                    p.ink_3,
                ))
                .child(kit::label(&p, "Completed"))
                .child(
                    div()
                        .style(ty::CAPS_LABEL)
                        .text_color(p.ink_3)
                        .child(count.to_string()),
                )
                .when(completed_enabled, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        this.ui.show_completed = !this.ui.show_completed;
                        this.refresh(cx);
                    }))
                })
        });
        div()
            .h(px(desktop::TOAST_INSET))
            .flex_none()
            .flex()
            .items_center()
            .overflow_hidden()
            .gap(px(space::S3))
            .px(px(desktop::LIST_PAD_X))
            .border_t_1()
            .border_color(p.line)
            .children(completed)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_end()
                    .children(self.toast_view(window, cx)),
            )
    }

    fn toast_view(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let look = self.toast.look(self.frame)?;
        let p = self.palette;
        let focused = self.undo_focus.is_focused(window);
        let label = |text, alpha| {
            div()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(p.toast_ink)
                .opacity(alpha)
                .child(text)
        };
        let button = div()
            .id("undo-task")
            .h_full()
            .flex_none()
            .flex()
            .gap(px(space::S1))
            .items_center()
            .px(px(space::S2))
            .rounded(px(radius::BUTTON))
            .style(ty::TOAST_ACTION)
            .text_color(p.toast_accent)
            .child(kit::icon(
                Icon::UndoTwo,
                component::ICON_SMALL,
                p.toast_accent,
            ))
            .child("Undo")
            .when(look.active, |d| {
                d.track_focus(&self.undo_focus)
                    .role(Role::Button)
                    .aria_label("Undo")
                    .aria_keyshortcuts("Control+Z")
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .border(px(elevation::HAIRLINE))
                    .border_color(if focused {
                        p.toast_accent
                    } else {
                        crate::theme::with_alpha(p.toast, 0.)
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.list_focus, cx);
                        this.request_undo(cx);
                    }))
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                        if e.keystroke.key == "tab"
                            && !e.keystroke.modifiers.control
                            && !e.keystroke.modifiers.alt
                            && !e.keystroke.modifiers.platform
                        {
                            cx.stop_propagation();
                            if !e.is_held {
                                window.focus(&this.list_focus, cx);
                            }
                        } else if !e.keystroke.modifiers.modified()
                            && matches!(e.keystroke.key.as_str(), "enter" | "space" | "escape")
                        {
                            cx.stop_propagation();
                            if !e.is_held {
                                window.focus(&this.list_focus, cx);
                                if e.keystroke.key == "escape" {
                                    this.toast.dismiss(this.now(), this.pace());
                                    cx.notify();
                                } else {
                                    this.request_undo(cx);
                                }
                            }
                        }
                    }))
            });
        let dismiss = div()
            .id("dismiss-undo")
            .h_full()
            .flex_none()
            .px(px(space::S2))
            .flex()
            .items_center()
            .child(kit::icon(Icon::X, component::ICON_SMALL, p.toast_ink_2))
            .when(look.active, |d| {
                d.role(Role::Button)
                    .aria_label("Dismiss undo toast")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.undo_focus.is_focused(window) {
                            window.focus(&this.list_focus, cx);
                        }
                        this.toast.dismiss(this.now(), this.pace());
                        cx.notify();
                    }))
            });
        Some(
            div()
                .w_full()
                .max_w(px(desktop::DETAIL))
                .h(px(desktop::TOAST))
                .relative()
                .top(px(look.y))
                .rounded(px(radius::TOAST))
                .overflow_hidden()
                .bg(p.toast)
                .opacity(look.alpha)
                .child(
                    div()
                        .id("undo-toast")
                        .size_full()
                        .flex()
                        .items_center()
                        .pl(px(space::S3))
                        .pr(px(space::S1))
                        .gap(px(space::S1))
                        .style(ty::TOAST)
                        .when(look.active, |d| {
                            d.on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                this.toast_hover = *hovered;
                                cx.notify();
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        })
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .child(label(look.text.clone(), look.swap))
                                .children(look.previous.map(|text| {
                                    label(text.clone(), 1. - look.swap)
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top_0()
                                })),
                        )
                        .child(button)
                        .child(dismiss),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w(gpui::relative(look.left))
                        .h(px(desktop::TOAST_COUNTDOWN))
                        .bg(p.toast_ink_2),
                ),
        )
    }
}
