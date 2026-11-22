//! Undo target ownership and the toast in the list's reserved footer. The target outlives the toast.

use std::sync::Arc;

use gpui::{Context, Div, KeyDownEvent, MouseButton, Role, SharedString, div, prelude::*, px};
use todo_client::{CommandId, TrackedUndo};
use todo_core::ids::{EntityId, TaskId};
use todo_core::{Command, State};

use super::{
    Icon, Notice, RoomKey, ShellView, component, desktop, elevation, kit, radius, space, ty,
};
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

    pub(super) fn label(self, title: &str) -> SharedString {
        task_label(self.verb(), title)
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Complete => "Completed",
            Self::Delete => "Deleted",
            Self::Move => "Moved",
        }
    }

    fn undone_label(self, title: &str) -> SharedString {
        task_label(
            match self {
                Self::Complete => "Reopened",
                Self::Delete => "Restored",
                Self::Move => "Moved back",
            },
            title,
        )
    }

    fn needs_row_restore(self) -> bool {
        matches!(self, Self::Complete | Self::Delete)
    }

    fn is_restored(self, id: TaskId, state: &State) -> bool {
        state.is_live(id.into())
            && (self != Self::Complete || state.tasks.get(&id).is_some_and(|t| !t.is_done()))
    }

    fn undo_changed_view(self, id: TaskId, before: &State, after: &State) -> bool {
        if self == Self::Move {
            before
                .tasks
                .get(&id)
                .zip(after.tasks.get(&id))
                .is_some_and(|(old, new)| old.list != new.list || old.position != new.position)
        } else {
            !self.is_restored(id, before) && self.is_restored(id, after)
        }
    }
}

fn task_label(verb: &str, title: &str) -> SharedString {
    format!("{verb} \u{201c}{}\u{201d}", one_line(title)).into()
}

/// Both directions can reach Undo without stealing system or application Tab shortcuts.
pub(super) fn undo_tab(event: &KeyDownEvent) -> bool {
    let key = &event.keystroke;
    key.key == "tab"
        && !key.modifiers.control
        && !key.modifiers.alt
        && !key.modifiers.platform
        && !key.modifiers.function
}

/// A later local edit makes the old one-step undo target stale, including edits to a task's children.
pub(super) fn touches_task(command: &Command, id: TaskId, state: &State) -> bool {
    let entity_touches = |entity| match entity {
        EntityId::Task(task) => task == id,
        EntityId::Step(step) => state.steps.get(&step).is_some_and(|s| s.task == id),
        EntityId::Link(link) => state.links.get(&link).is_some_and(|l| l.task == id),
        EntityId::Attachment(file) => state.attachments.get(&file).is_some_and(|a| a.task == id),
        EntityId::Reminder(reminder) => {
            state.reminders.get(&reminder).is_some_and(|r| r.task == id)
        }
        _ => false,
    };
    match *command {
        Command::UpdateTask { id: task, .. }
        | Command::MoveTask { id: task, .. }
        | Command::CompleteTask { id: task }
        | Command::UncompleteTask { id: task }
        | Command::SetMyDay { id: task, .. }
        | Command::AddTag { task, .. }
        | Command::RemoveTag { task, .. }
        | Command::AddNote { task, .. }
        | Command::CreateStep { task, .. }
        | Command::AddLink { task, .. }
        | Command::AddAttachment { task, .. }
        | Command::AddReminder { task, .. } => task == id,
        Command::MergeTasks { source, target } => source == id || target == id,
        Command::UpdateStep { id, .. }
        | Command::CompleteStep { id }
        | Command::UncompleteStep { id } => entity_touches(id.into()),
        Command::UpdateReminder { id, .. } => entity_touches(id.into()),
        Command::Reorder { id, .. } | Command::Delete { id } | Command::Restore { id } => {
            entity_touches(id)
        }
        _ => false,
    }
}

#[derive(Debug)]
pub(super) struct Action {
    pub id: TaskId,
    pub kind: EditKind,
    pub target: UndoTarget,
    pub room: RoomKey,
}

impl Action {
    pub(super) fn is_in(&self, room: RoomKey) -> bool {
        self.room == room
    }
}

#[derive(Debug)]
pub(super) struct Pending {
    id: TaskId,
    kind: EditKind,
    undo_id: CommandId,
    animated: bool,
    reported: bool,
}

impl ShellView {
    pub(super) fn remember_undo(&mut self, action: Action, title: &str) {
        let (now, pace) = (self.now(), self.pace());
        self.toast.advance(now, pace);
        self.toast.show(action.kind.label(title), now, pace);
        self.last_undo = Some(action);
    }

    pub(super) fn clear_undo(&mut self) {
        self.last_undo = None;
        self.toast.dismiss(self.now(), self.pace());
    }

    pub(super) fn request_undo(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.last_undo.take() else {
            return;
        };
        let (now, pace) = (self.now(), self.pace());
        self.toast.advance(now, pace);
        let toast_active = self.toast.look(now).is_some_and(|t| t.active);
        let before = (!toast_active).then(|| self.store.view());
        let Action { id, kind, .. } = action;
        let queued = match self.store.undo(&action.target) {
            Err(error) => {
                // In particular a cache write failure must leave the action retryable.
                self.notice = Some(Notice::error(format!("Couldn't undo that: {error}")));
                self.last_undo = Some(action);
                cx.notify();
                return;
            }
            Ok(TrackedUndo::Unavailable) => {
                self.notice = Some(Notice::error("That change is no longer available to undo."));
                self.toast.dismiss(now, pace);
                self.refresh(cx);
                return;
            }
            Ok(TrackedUndo::Cancelled) => None,
            Ok(TrackedUndo::Queued(undo_id)) => Some(undo_id),
        };
        let state = self.store.view();
        let animated = !kind.needs_row_restore() || self.animate_restored(kind, id, &state);
        if let Some(undo_id) = queued {
            self.pending_undo.push(Pending {
                id,
                kind,
                undo_id,
                animated,
                reported: false,
            });
        }
        self.notice = before.map(|before| {
            if kind.undo_changed_view(id, &before, &state) {
                let title = state
                    .tasks
                    .get(&id)
                    .map_or("Task", |task| task.title.as_str());
                Notice::info(kind.undone_label(title))
            } else if queued.is_some() {
                Notice::info("Undo queued.")
            } else {
                Notice::info("Change cancelled.")
            }
        });
        self.toast.dismiss(now, pace);
        self.refresh(cx);
    }

    fn animate_restored(&mut self, kind: EditKind, id: TaskId, state: &State) -> bool {
        if self.animate
            && kind.is_restored(id, state)
            && let Some(task) = state.tasks.get(&id)
        {
            self.motion
                .restore(id, task.is_done(), self.now(), self.pace());
            return true;
        }
        false
    }

    /// Animate the compensation once using the same view that also decides new commands.
    pub(super) fn undo_state(&mut self) -> Arc<State> {
        let state = self.store.view();
        for mut pending in std::mem::take(&mut self.pending_undo) {
            match self.store.undo_result(pending.undo_id) {
                Some(Err(message)) => {
                    if !pending.reported {
                        self.notice = Some(Notice::error(message));
                        pending.reported = true;
                    }
                    if !self.store.dismiss_undo_failure(pending.undo_id) {
                        self.pending_undo.push(pending);
                    }
                }
                result => {
                    if !pending.animated {
                        pending.animated = self.animate_restored(pending.kind, pending.id, &state);
                    }
                    if result.is_none() {
                        self.pending_undo.push(pending);
                    }
                }
            }
        }
        state
    }

    /// This footer is outside the scroll viewport and the narrow detail overlay. The overlay
    /// keeps Completed inert, but the same Undo is always visible and reachable.
    pub(super) fn list_footer(&self, completed_enabled: bool, cx: &mut Context<Self>) -> Div {
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
                    .children(self.toast_view(cx)),
            )
    }

    fn toast_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let look = self.toast.look(self.frame)?;
        let p = self.palette;
        let focus = self.undo_ring.at(self.frame);
        let actionable = look.active && (self.last_undo.is_some() || !self.animate);
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
            .py(px(space::S1))
            .child(
                div()
                    .h_full()
                    .p(px(elevation::FOCUS_RING))
                    .rounded(px(radius::BUTTON + elevation::FOCUS_RING))
                    .bg(crate::theme::with_alpha(p.tint, p.tint.a * focus))
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .gap(px(space::S1))
                            .items_center()
                            .px(px(space::S2))
                            .rounded(px(radius::BUTTON))
                            .border(px(elevation::HAIRLINE))
                            .border_color(crate::theme::with_alpha(p.accent, focus))
                            .bg(p.toast)
                            .style(ty::TOAST_ACTION)
                            .text_color(p.toast_accent)
                            .child(kit::icon(
                                Icon::UndoTwo,
                                component::ICON_SMALL,
                                p.toast_accent,
                            ))
                            .child("Undo"),
                    ),
            )
            .when(actionable, |d| {
                d.track_focus(&self.undo_focus)
                    .role(Role::Button)
                    .aria_label("Undo")
                    .aria_keyshortcuts("Control+Z")
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.list_focus, cx);
                        this.request_undo(cx);
                    }))
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                        if undo_tab(e) {
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
                .max_w(px(desktop::TOAST_MAX))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, model::Smart};
    use gpui::Keystroke;
    use todo_core::command::TaskPatch;

    #[test]
    fn the_undo_target_belongs_only_to_the_room_that_created_it() {
        let room = RoomKey::Smart(Smart::MyDay);
        let fixture = fixtures::my_day();
        let id = fixture.ui.task.expect("selected task");
        let target = crate::store::Store::fixture(&fixture)
            .submit_undoable(Command::CompleteTask { id })
            .expect("completion");
        let action = Action {
            id,
            kind: EditKind::Complete,
            target,
            room,
        };
        assert!(action.is_in(room));
        assert!(!action.is_in(RoomKey::List(fixture.state.tasks[&id].list)));
    }

    #[test]
    fn a_later_reopen_edit_or_step_edit_invalidates_only_the_tasks_own_target() {
        let fixture = fixtures::my_day();
        let id = fixture.ui.task.expect("selected task");
        let another = fixture
            .state
            .tasks
            .keys()
            .copied()
            .find(|task| *task != id)
            .expect("another task");
        let step = fixture
            .state
            .steps
            .values()
            .find(|step| step.task == id)
            .expect("step");
        for command in [
            Command::UncompleteTask { id },
            Command::UpdateTask {
                id,
                patch: TaskPatch {
                    title: Some("Renamed".into()),
                    ..TaskPatch::default()
                },
            },
            Command::CompleteStep { id: step.id },
        ] {
            assert!(touches_task(&command, id, &fixture.state));
            assert!(!touches_task(&command, another, &fixture.state));
        }
    }

    #[test]
    fn only_a_completion_or_deletion_undo_restores_the_row() {
        assert!(EditKind::Complete.needs_row_restore());
        assert!(EditKind::Delete.needs_row_restore());
        assert!(!EditKind::Move.needs_row_restore());
    }

    #[test]
    fn undo_focus_is_reachable_from_both_tab_directions() {
        let key = |binding| KeyDownEvent {
            keystroke: Keystroke::parse(binding).expect("keystroke"),
            is_held: false,
            prefer_character_input: false,
        };
        assert!(undo_tab(&key("tab")));
        assert!(undo_tab(&key("shift-tab")));
        assert!(!undo_tab(&key("ctrl-tab")));
        assert!(!undo_tab(&key("alt-tab")));
    }

    #[test]
    fn the_gallery_uses_the_same_title_formatter_as_a_real_completion() {
        for name in ["undo-toast", "narrow-undo-toast"] {
            let fixture = fixtures::by_name(name).expect("undo fixture");
            let Some(fixtures::Moment::UndoToast { task, .. }) = fixture.moment else {
                panic!("expected a completion toast");
            };
            assert!(fixture.state.tasks[&task].is_done());
            assert_eq!(
                EditKind::Complete
                    .label(&fixture.state.tasks[&task].title)
                    .as_ref(),
                "Completed “Send venue booking confirmation”"
            );
        }
        assert_eq!(
            EditKind::Move.undone_label("A\nB").as_ref(),
            "Moved back “A B”"
        );
    }
}
