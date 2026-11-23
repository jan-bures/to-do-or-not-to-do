//! Undo target ownership and the toast in the list's reserved footer. The target outlives the toast.

use std::sync::Arc;

use gpui::{Context, Div, MouseButton, Role, SharedString, div, prelude::*, px};
use todo_client::{CommandId, TrackedUndo};
use todo_core::ids::{CollectionId, EntityId, TaskId};
use todo_core::model::Entity;
use todo_core::{Command, State};

use super::{
    Icon, Notice, RoomKey, ShellView, component, desktop, elevation, kit, radius, space, ty,
};
use crate::model::labels::one_line;
use crate::store::UndoTarget;
use crate::theme::TextStyled;

/// What an undoable change did: a task change, adding or deleting a link, deleting a step, a list or a
/// whole collection, or moving a list to another collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Complete,
    Restore,
    Delete,
    Merge,
    Move,
    DeleteStep,
    DeleteTag,
    Repeat,
    ClearDue,
    /// A reminder set or moved (the picker's preset or Set reminder).
    SetReminder,
    RemoveReminder,
    AddLink,
    DeleteLink,
    DeleteAttachment,
    DeleteList,
    MoveList,
    DeleteCollection,
}

impl EditKind {
    /// The changed entity and the kind of change, for a command the toast can undo.
    pub(super) fn of(command: &Command) -> Option<(EntityId, Self)> {
        Some(match *command {
            Command::CompleteTask { id } => (id.into(), Self::Complete),
            Command::Restore { id } => (id, Self::Restore),
            Command::Delete { id } => match id {
                EntityId::Task(_) => (id, Self::Delete),
                EntityId::Step(_) => (id, Self::DeleteStep),
                EntityId::Tag(_) => (id, Self::DeleteTag),
                EntityId::Link(_) => (id, Self::DeleteLink),
                EntityId::Attachment(_) => (id, Self::DeleteAttachment),
                EntityId::List(_) => (id, Self::DeleteList),
                EntityId::Collection(_) => (id, Self::DeleteCollection),
                EntityId::Reminder(_) => (id, Self::RemoveReminder),
            },
            Command::UpdateTask { id, ref patch } if patch.recurrence.is_some() => (
                id.into(),
                if patch.due == Some(None) {
                    Self::ClearDue
                } else {
                    Self::Repeat
                },
            ),
            Command::MoveTask { id, .. } => (id.into(), Self::Move),
            Command::MergeTasks { source, .. } => (source.into(), Self::Merge),
            Command::AddReminder { id, .. } | Command::UpdateReminder { id, .. } => {
                (id.into(), Self::SetReminder)
            }
            Command::MoveList { id, .. } => (id.into(), Self::MoveList),
            // Undoing it withdraws the link: Ctrl+Z right after adding one takes that one back.
            Command::AddLink { id, .. } => (id.into(), Self::AddLink),
            Command::Reorder {
                id: id @ EntityId::Task(_),
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
            Self::Restore => "Restored",
            Self::Delete => "Deleted",
            Self::Merge => "Merged",
            Self::Move | Self::MoveList => "Moved",
            Self::DeleteStep => "Deleted step",
            Self::DeleteTag => "Deleted tag",
            Self::Repeat => "Changed schedule for",
            Self::ClearDue => "Cleared due date for",
            Self::SetReminder => "Set reminder for",
            Self::RemoveReminder => "Removed reminder for",
            Self::AddLink => "Added link",
            Self::DeleteLink => "Removed link",
            Self::DeleteAttachment => "Removed attachment",
            Self::DeleteList => "Deleted list",
            Self::DeleteCollection => "Deleted collection",
        }
    }

    fn undone_label(self, title: &str) -> SharedString {
        task_label(
            match self {
                Self::Complete => "Reopened",
                Self::Restore => "Returned to Trash:",
                Self::Delete | Self::Merge => "Restored",
                Self::Move | Self::MoveList => "Moved back",
                Self::DeleteStep => "Restored step",
                Self::DeleteTag => "Restored tag",
                Self::Repeat | Self::ClearDue => "Restored schedule for",
                Self::SetReminder | Self::RemoveReminder => "Restored reminder for",
                Self::AddLink => "Removed link",
                Self::DeleteLink => "Restored link",
                Self::DeleteAttachment => "Restored attachment",
                Self::DeleteList => "Restored list",
                Self::DeleteCollection => "Restored collection",
            },
            title,
        )
    }

    /// The change took a task's row away, and undoing it brings the row back with an entrance.
    fn needs_row_restore(self) -> bool {
        matches!(self, Self::Complete | Self::Delete | Self::Merge)
    }

    fn is_restored(self, entity: EntityId, state: &State) -> bool {
        let open = || match entity {
            EntityId::Task(id) => state.tasks.get(&id).is_some_and(|t| !t.is_done()),
            _ => true,
        };
        state.is_live(entity) && (self != Self::Complete || open())
    }

    fn undo_changed_view(self, entity: EntityId, before: &State, after: &State) -> bool {
        match (self, entity) {
            (Self::Repeat | Self::ClearDue, EntityId::Task(id)) => before
                .tasks
                .get(&id)
                .zip(after.tasks.get(&id))
                .is_some_and(|(a, b)| (a.due, &a.recurrence) != (b.due, &b.recurrence)),
            (Self::Move, EntityId::Task(id)) => before
                .tasks
                .get(&id)
                .zip(after.tasks.get(&id))
                .is_some_and(|(old, new)| old.list != new.list || old.position != new.position),
            (Self::MoveList, EntityId::List(id)) => before
                .lists
                .get(&id)
                .zip(after.lists.get(&id))
                .is_some_and(|(old, new)| {
                    old.collection != new.collection || old.position != new.position
                }),
            (Self::Move | Self::MoveList, _) => false,
            // Its time back, or the reminder taken away again (undoing one added).
            (Self::SetReminder, EntityId::Reminder(id)) => {
                let at = |s: &State| {
                    s.reminders
                        .get(&id)
                        .filter(|_| s.is_live(entity))
                        .map(|r| r.at)
                };
                at(before) != at(after)
            }
            // Undoing an addition withdraws the link.
            (Self::AddLink, _) => before.is_live(entity) && !after.is_live(entity),
            (Self::Restore, _) => {
                before.tombstone(entity).is_none() && after.tombstone(entity).is_some()
            }
            _ => !self.is_restored(entity, before) && self.is_restored(entity, after),
        }
    }

    /// The change took the room the window showed away with it (deleting its list or collection, or
    /// the collection its list is in): the window goes to My Day, and the undo goes along.
    pub(super) fn leaves_room(self) -> bool {
        matches!(self, Self::DeleteList | Self::DeleteCollection)
    }
}

/// Why an undone delete still shows nothing. Undo is the exact inverse of the delete: it brings back the
/// task, step or list itself, never what it belongs to. If that (its list, task or collection) was
/// deleted meanwhile, on another device or by an agent, the restored item stays hidden with it, and the
/// notice says so rather than leave Undo looking as if it did nothing.
pub(super) fn hidden_by_parent(kind: EditKind, entity: EntityId, state: &State) -> Option<String> {
    if !matches!(
        kind,
        EditKind::Delete
            | EditKind::Merge
            | EditKind::DeleteStep
            | EditKind::DeleteLink
            | EditKind::DeleteAttachment
            | EditKind::DeleteList
    ) || state.tombstone(entity).is_some()
        || state.is_live(entity)
    {
        return None;
    }
    // The nearest deleted ancestor is what hides it.
    let mut at = state.entity(entity)?.parent();
    let gone = loop {
        let parent = state.entity(at?)?;
        if parent.deleted().is_some() {
            break parent;
        }
        at = parent.parent();
    };
    let what = match gone {
        Entity::Task(_) => "task",
        Entity::List(_) => "list",
        Entity::Collection(_) => "collection",
        _ => return None,
    };
    let subject = match kind {
        EditKind::DeleteStep => "Step",
        EditKind::DeleteLink => "Link",
        EditKind::DeleteAttachment => "Attachment",
        EditKind::DeleteList => "List",
        _ => "Task",
    };
    // No names: the status bar is one short line, and a long title would push the explanation out.
    Some(format!(
        "{subject} restored, but its {what} was deleted elsewhere."
    ))
}

fn task_label(verb: &str, title: &str) -> SharedString {
    format!("{verb} \u{201c}{}\u{201d}", one_line(title)).into()
}

/// What the toast says a change did: Deleted “Buy stamps”, or for a move, where it went: Moved “Buy
/// stamps” to Errands, Moved “Garden” to Home, or Moved “Garden” out of Home. `state` is the state
/// before the change.
pub(super) fn toast_label(
    state: &State,
    command: &Command,
    entity: EntityId,
    kind: EditKind,
) -> SharedString {
    let title = subject_title(state, entity);
    let moved = |to: &str| -> SharedString {
        format!("Moved \u{201c}{}\u{201d} {to}", one_line(&title)).into()
    };
    match (command, kind) {
        (Command::MergeTasks { source, target }, EditKind::Merge) => {
            todo_client::intent::merge::preview(state, *source, *target)
                .map_or_else(|| kind.label(&title), |p| p.toast.into())
        }
        (Command::MoveTask { list, .. }, EditKind::Move) => match state.lists.get(list) {
            Some(list) => moved(&format!("to {}", one_line(&list.name))),
            None => kind.label(&title),
        },
        // The link isn't in the state before it's added: the command names it.
        (Command::AddLink { kind: link, .. }, EditKind::AddLink) => {
            kind.label(&crate::model::menus::link_name(link))
        }
        // Nor is a new reminder: it is named by its task.
        (Command::AddReminder { task, .. }, EditKind::SetReminder) => {
            kind.label(&subject_title(state, (*task).into()))
        }
        (Command::MoveList { id, collection, .. }, EditKind::MoveList) => {
            let name = |c: &CollectionId| state.collections.get(c).map(|c| one_line(&c.name));
            match (collection, state.lists.get(id).and_then(|l| l.collection)) {
                (Some(to), _) => match name(to) {
                    Some(to) => moved(&format!("to {to}")),
                    None => kind.label(&title),
                },
                (None, Some(from)) => match name(&from) {
                    Some(from) => moved(&format!("out of {from}")),
                    None => kind.label(&title),
                },
                (None, None) => kind.label(&title),
            }
        }
        _ => kind.label(&title),
    }
}

/// What the toast names: a task's or a step's title, a link's title (or address), or a list's or a
/// collection's name.
pub(super) fn subject_title(state: &State, entity: EntityId) -> String {
    match entity {
        EntityId::Task(id) => state.tasks.get(&id).map(|t| t.title.clone()),
        EntityId::Step(id) => state.steps.get(&id).map(|s| s.title.clone()),
        EntityId::Tag(id) => state.tags.get(&id).map(|t| t.name.clone()),
        EntityId::Link(id) => {
            (state.links.get(&id)).map(|l| crate::model::menus::link_name(&l.kind))
        }
        EntityId::Attachment(id) => state.attachments.get(&id).map(|a| a.file_name.clone()),
        EntityId::List(id) => state.lists.get(&id).map(|l| l.name.clone()),
        EntityId::Collection(id) => state.collections.get(&id).map(|c| c.name.clone()),
        // A reminder is named by its task.
        EntityId::Reminder(id) => (state.reminders.get(&id))
            .and_then(|r| state.tasks.get(&r.task))
            .map(|t| t.title.clone()),
    }
    .unwrap_or_else(|| "Task".into())
}

/// A later local edit makes the old one-step undo target stale: for a task, an edit to it or to its
/// children; for a step, a link, a list or a collection, an edit to that one.
pub(super) fn touches(command: &Command, entity: EntityId, state: &State) -> bool {
    match entity {
        EntityId::Task(id) => touches_task(command, id, state),
        _ => command_target(command) == Some(entity),
    }
}

fn command_target(command: &Command) -> Option<EntityId> {
    Some(match *command {
        Command::UpdateStep { id, .. }
        | Command::CompleteStep { id }
        | Command::UncompleteStep { id } => id.into(),
        Command::UpdateList { id, .. } | Command::MoveList { id, .. } => id.into(),
        Command::UpdateCollection { id, .. } => id.into(),
        Command::UpdateTag { id, .. } => id.into(),
        Command::UpdateReminder { id, .. } => id.into(),
        Command::Reorder { id, .. } | Command::Delete { id } | Command::Restore { id } => id,
        _ => return None,
    })
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
    pub entity: EntityId,
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
    entity: EntityId,
    kind: EditKind,
    undo_id: CommandId,
    animated: bool,
    reported: bool,
}

impl ShellView {
    pub(super) fn remember_undo(&mut self, action: Action, label: SharedString) {
        let (now, pace) = (self.now(), self.pace());
        self.toast.advance(now, pace);
        self.toast.show(label, now, pace);
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
        let Action { entity, kind, .. } = action;
        // Named as it is now: undoing an addition takes the link away with its name.
        let title = subject_title(&self.store.view(), entity);
        let queued = match self.store.undo(&action.target) {
            Err(error) => {
                // In particular a cache write failure must leave the action retryable.
                self.notice = Some(Notice::error(error));
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
        // A task moved back while its row still leaves comes back from where it is; one moved back
        // into this room from another list enters it.
        if kind == EditKind::Move
            && self.animate
            && let EntityId::Task(id) = entity
            && (self.motion.is_leaving(id) || self.shell.shown_row(id).is_none())
            && let Some(task) = state.tasks.get(&id)
        {
            self.restore_row(id, task.is_done());
        }
        let animated = !kind.needs_row_restore() || self.animate_restored(kind, entity, &state);
        if let Some(undo_id) = queued {
            self.pending_undo.push(Pending {
                entity,
                kind,
                undo_id,
                animated,
                reported: false,
            });
        }
        let hidden = hidden_by_parent(kind, entity, &state);
        self.notice = hidden.map(Notice::info).or_else(|| {
            before.map(|before| {
                if kind.undo_changed_view(entity, &before, &state) {
                    Notice::info(kind.undone_label(&title))
                } else if queued.is_some() {
                    Notice::info("Undo queued.")
                } else {
                    Notice::info("Change cancelled.")
                }
            })
        });
        self.toast.dismiss(now, pace);
        self.refresh(cx);
    }

    fn animate_restored(&mut self, kind: EditKind, entity: EntityId, state: &State) -> bool {
        if self.animate
            && let EntityId::Task(id) = entity
            && kind.is_restored(entity, state)
            && let Some(task) = state.tasks.get(&id)
        {
            self.restore_row(id, task.is_done());
            return true;
        }
        false
    }

    /// Brings a restored task's row back (`Motion::restore`). A completion or reopening its delete cut
    /// short picks up again: a reopening keeps the completed place it had (`UiState::reopening`, from
    /// the task as it was when it left); a completion is held in the open rows again by the motion.
    fn restore_row(&mut self, id: TaskId, done: bool) {
        let completed = self.ui.leaving.get(&id).and_then(|t| t.completed_at);
        self.motion.restore(id, done, self.now(), self.pace());
        if let Some(at) = completed
            && self.motion.reopening().any(|r| r == id)
        {
            self.ui.reopening.insert(id, at);
        }
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
                        pending.animated =
                            self.animate_restored(pending.kind, pending.entity, &state);
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
                        if !e.keystroke.modifiers.modified()
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
            entity: id.into(),
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
    fn an_undone_delete_hidden_by_a_parent_deleted_elsewhere_says_so() {
        use crate::store::Store;
        // A step, then its task deleted on another device: undoing the step's delete brings back exactly
        // the step, which stays hidden with its task, and the notice says why.
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let step = f
            .state
            .steps
            .values()
            .find(|s| s.task == task && s.deleted.is_none())
            .expect("step")
            .id;
        let store = Store::fixture(&f);
        let target = store
            .submit_undoable(Command::Delete { id: step.into() })
            .expect("delete step");
        store
            .submit(Command::Delete { id: task.into() })
            .expect("the task deleted elsewhere");
        store.undo(&target).expect("exact inverse");
        let state = store.view();
        assert!(
            state.steps[&step].deleted.is_none(),
            "the step itself is back"
        );
        assert!(state.tasks[&task].deleted.is_some(), "its task is not");
        // A short message on its own, without names, so a long title can't hide the explanation.
        assert_eq!(
            hidden_by_parent(EditKind::DeleteStep, step.into(), &state).as_deref(),
            Some("Step restored, but its task was deleted elsewhere.")
        );

        // A list, then its collection.
        let f = fixtures::work();
        let list = f
            .state
            .lists
            .values()
            .find(|l| l.collection.is_some() && l.deleted.is_none())
            .expect("a list in a collection");
        let (list, collection) = (list.id, list.collection.expect("collection"));
        let store = Store::fixture(&f);
        let target = store
            .submit_undoable(Command::Delete { id: list.into() })
            .expect("delete list");
        store
            .submit(Command::Delete {
                id: collection.into(),
            })
            .expect("the collection deleted elsewhere");
        store.undo(&target).expect("exact inverse");
        let state = store.view();
        assert!(state.lists[&list].deleted.is_none());
        assert_eq!(
            hidden_by_parent(EditKind::DeleteList, list.into(), &state).as_deref(),
            Some("List restored, but its collection was deleted elsewhere.")
        );

        // Nothing to explain when the restored item shows, or when it's still deleted itself.
        let f = fixtures::my_day();
        let store = Store::fixture(&f);
        let target = store
            .submit_undoable(Command::Delete { id: step.into() })
            .expect("delete step");
        assert_eq!(
            hidden_by_parent(EditKind::DeleteStep, step.into(), &store.view()),
            None
        );
        store.undo(&target).expect("undo");
        assert_eq!(
            hidden_by_parent(EditKind::DeleteStep, step.into(), &store.view()),
            None
        );
        // Other kinds of change never hide anything this way.
        assert_eq!(
            hidden_by_parent(EditKind::Complete, task.into(), &store.view()),
            None
        );
    }

    #[test]
    fn the_toast_says_a_task_was_deleted_or_where_it_was_moved() {
        let f = fixtures::my_day();
        let id = f.ui.task.expect("selected task");
        let title = f.state.tasks[&id].title.clone();
        let errands = f
            .state
            .lists
            .values()
            .find(|l| l.name == "Errands")
            .expect("Errands")
            .id;
        let delete = Command::Delete { id: id.into() };
        let (entity, kind) = EditKind::of(&delete).expect("undoable");
        assert_eq!(kind, EditKind::Delete);
        assert_eq!(
            toast_label(&f.state, &delete, entity, kind).as_ref(),
            format!("Deleted \u{201c}{title}\u{201d}")
        );
        let moved = crate::model::commands::move_to_list(&f.state, id, errands).expect("move");
        let (entity, kind) = EditKind::of(&moved).expect("undoable");
        assert_eq!(kind, EditKind::Move);
        assert_eq!(
            toast_label(&f.state, &moved, entity, kind).as_ref(),
            format!("Moved \u{201c}{title}\u{201d} to Errands")
        );
        // Moving within its list says only that it moved.
        let nudge = crate::model::commands::nudge(&f.state, id.into(), 1).expect("reorder");
        let (entity, kind) = EditKind::of(&nudge).expect("undoable");
        assert_eq!(
            toast_label(&f.state, &nudge, entity, kind).as_ref(),
            format!("Moved \u{201c}{title}\u{201d}")
        );
        assert_eq!(
            EditKind::Delete.undone_label(&title).as_ref(),
            format!("Restored \u{201c}{title}\u{201d}")
        );
    }

    #[test]
    fn undoing_a_new_repeat_removes_the_due_date_it_added() {
        use crate::store::Store;
        use todo_client::intent::repeat::{Preset, set_repeat};
        let f = fixtures::by_name("repeat-none").expect("fixture");
        let task = f.ui.task.expect("task");
        let today = f.now.today();
        let cmd = set_repeat(&f.state, task, Some(Preset::Weekdays.rule(today)), today)
            .expect("valid")
            .expect("changed");
        let (entity, kind) = EditKind::of(&cmd).expect("tracked");
        assert_eq!(kind, EditKind::Repeat);
        let store = Store::fixture(&f);
        let target = store.submit_undoable(cmd).expect("repeat");
        let after = store.view();
        assert_eq!(after.tasks[&task].due, todo_core::time::add_days(today, 2)); // Saturday → Monday
        assert!(after.tasks[&task].recurrence.is_some());
        store.undo(&target).expect("undone");
        let back = store.view();
        assert_eq!(
            (back.tasks[&task].due, &back.tasks[&task].recurrence),
            (None, &None)
        );
        assert!(kind.undo_changed_view(entity, &after, &back));
    }

    #[test]
    fn undoing_a_reminder_change_puts_back_exactly_what_was_there() {
        use crate::store::Store;
        use todo_client::intent::reminder::{Seen, remove_reminder, set_reminder, shown};
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let title = f.state.tasks[&task].title.clone();
        let ctx = f.now.eval();
        let seen = Seen::of(&f.state, task, &ctx).expect("the fixture's task has one");
        let store = Store::fixture(&f);
        let at: todo_core::time::Timestamp = "2026-11-22T09:00:00Z".parse().expect("ts");
        // Moved: the toast names the task, and Undo moves it back.
        let moved = set_reminder(&store.view(), task, Some(seen), at, || store.new_uuid())
            .expect("unchanged")
            .expect("moved");
        let (entity, kind) = EditKind::of(&moved).expect("undoable");
        assert_eq!((entity, kind), (seen.id.into(), EditKind::SetReminder));
        assert_eq!(
            toast_label(&store.view(), &moved, entity, kind).as_ref(),
            format!("Set reminder for \u{201c}{title}\u{201d}")
        );
        let target = store.submit_undoable(moved).expect("moved");
        let after = store.view();
        assert_eq!(after.reminders[&seen.id].at, at);
        store.undo(&target).expect("undone");
        let back = store.view();
        assert_eq!(back.reminders[&seen.id].at, seen.at);
        assert!(kind.undo_changed_view(entity, &after, &back));
        // Removed: Undo brings the same reminder back.
        let removed = remove_reminder(&back, task, seen).expect("removed");
        let (entity, kind) = EditKind::of(&removed).expect("undoable");
        assert_eq!(kind, EditKind::RemoveReminder);
        assert_eq!(
            toast_label(&back, &removed, entity, kind).as_ref(),
            format!("Removed reminder for \u{201c}{title}\u{201d}")
        );
        let target = store.submit_undoable(removed).expect("removed");
        let after = store.view();
        assert!(shown(&after, task, &ctx).is_none());
        store.undo(&target).expect("undone");
        let back = store.view();
        assert_eq!(
            shown(&back, task, &ctx).map(|r| (r.id, r.at)),
            Some((seen.id, seen.at))
        );
        assert!(kind.undo_changed_view(entity, &after, &back));
        // Added to a task that had none: the toast names the task, and Undo takes it away.
        let bare = (back.tasks.values())
            .find(|t| back.is_live(t.id.into()) && shown(&back, t.id, &ctx).is_none())
            .map(|t| (t.id, t.title.clone()))
            .expect("a task without a reminder");
        let added = set_reminder(&back, bare.0, None, at, || store.new_uuid())
            .expect("unchanged")
            .expect("added");
        let (entity, kind) = EditKind::of(&added).expect("undoable");
        assert_eq!(
            toast_label(&back, &added, entity, kind).as_ref(),
            format!("Set reminder for \u{201c}{}\u{201d}", bare.1)
        );
        let target = store.submit_undoable(added).expect("added");
        let after = store.view();
        store.undo(&target).expect("undone");
        let back = store.view();
        assert!(shown(&back, bare.0, &ctx).is_none());
        assert!(kind.undo_changed_view(entity, &after, &back));
    }

    #[test]
    fn clearing_due_on_a_repeat_has_its_own_toast() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected");
        let cmd = crate::model::commands::set_due(&f.state, task, None).expect("clear due");
        let (entity, kind) = EditKind::of(&cmd).expect("undoable");
        assert!(toast_label(&f.state, &cmd, entity, kind).starts_with("Cleared due date for"));
    }

    #[test]
    fn undoing_a_tag_delete_restores_the_tag_and_memberships() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let tag = *f.state.tasks[&task].tags.first().expect("tag");
        let cmd = crate::model::commands::delete(tag);
        let (entity, kind) = EditKind::of(&cmd).expect("tracked");
        assert_eq!(kind, EditKind::DeleteTag);
        let store = Store::fixture(&f);
        let target = store.submit_undoable(cmd).expect("delete");
        let after = store.view();
        assert!(!after.is_live(entity) && after.is_live(task.into()));
        store.undo(&target).expect("undone");
        let back = store.view();
        assert!(back.tasks[&task].tags.contains(&tag));
        assert_eq!(back.tags[&tag].name, f.state.tags[&tag].name);
        assert!(kind.undo_changed_view(entity, &after, &back));
    }

    #[test]
    fn deleting_a_collection_hides_its_lists_and_undo_brings_them_all_back() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let state = &f.state;
        let work = commands_collection(state, "Work");
        let lists: Vec<_> = (state.lists.values())
            .filter(|l| l.collection == Some(work))
            .map(|l| l.id)
            .collect();
        let delete = crate::model::commands::delete_collection(state, work).expect("delete");
        let (entity, kind) = EditKind::of(&delete).expect("undoable");
        assert_eq!((entity, kind), (work.into(), EditKind::DeleteCollection));
        assert!(kind.leaves_room());
        assert_eq!(
            toast_label(state, &delete, entity, kind).as_ref(),
            "Deleted collection “Work”"
        );
        let store = Store::fixture(&f);
        let target = store.submit_undoable(delete).expect("deleted");
        let after = store.view();
        assert!(lists.iter().all(|l| !after.is_live((*l).into())));
        // Only the collection carries the tombstone: its lists are hidden with it.
        assert!(lists.iter().all(|l| after.lists[l].deleted.is_none()));
        assert!(!kind.undo_changed_view(entity, &after, &after));
        store.undo(&target).expect("undone");
        let back = store.view();
        assert!(lists.iter().all(|l| back.is_live((*l).into())));
        assert!(kind.undo_changed_view(entity, &after, &back));
        assert_eq!(
            kind.undone_label(&subject_title(&back, entity)).as_ref(),
            "Restored collection “Work”"
        );
        assert_eq!(hidden_by_parent(kind, entity, &back), None);
    }

    fn commands_collection(state: &State, name: &str) -> todo_core::ids::CollectionId {
        state
            .collections
            .values()
            .find(|c| c.name == name)
            .map(|c| c.id)
            .expect("collection")
    }

    #[test]
    fn a_list_moved_to_another_collection_says_where_and_moves_back() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let state = &f.state;
        let personal = commands_collection(state, "Personal");
        let office = (state.lists.values())
            .find(|l| l.name == "Office")
            .expect("Office")
            .id;
        let into = crate::model::commands::move_list(state, office, Some(personal)).expect("move");
        let (entity, kind) = EditKind::of(&into).expect("undoable");
        assert_eq!((entity, kind), (office.into(), EditKind::MoveList));
        assert!(!kind.leaves_room() && !kind.needs_row_restore());
        assert_eq!(
            toast_label(state, &into, entity, kind).as_ref(),
            "Moved “Office” to Personal"
        );
        let out = crate::model::commands::move_list(state, office, None).expect("out");
        assert_eq!(
            toast_label(state, &out, entity, kind).as_ref(),
            "Moved “Office” out of Work"
        );
        let store = Store::fixture(&f);
        let target = store.submit_undoable(into).expect("moved");
        let moved = store.view();
        assert_eq!(moved.lists[&office].collection, Some(personal));
        store.undo(&target).expect("undone");
        let back = store.view();
        assert_eq!(back.lists[&office], state.lists[&office]);
        assert!(kind.undo_changed_view(entity, &moved, &back));
        assert!(!kind.undo_changed_view(entity, &back, &back));
        assert_eq!(
            kind.undone_label(&subject_title(&back, entity)).as_ref(),
            "Moved back “Office”"
        );
        // A later rename of the list makes the old target stale; a recolour of its collection doesn't.
        let rename = crate::model::commands::rename_list(&back, office, "Desk").expect("rename");
        assert!(touches(&rename, entity, &back));
        let recolour =
            crate::model::commands::set_color(&back, personal.into(), "teal").expect("colour");
        assert!(!touches(&recolour, entity, &back));
        assert!(touches(&recolour, personal.into(), &back));
    }

    #[test]
    fn undoing_an_added_link_takes_it_away_and_is_the_latest_undo() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let store = Store::fixture(&f);
        let add =
            crate::model::commands::add_link(&f.state, task, "docs.example.com/brief", || {
                store.new_uuid()
            })
            .expect("a link");
        let (entity, kind) = EditKind::of(&add).expect("undoable");
        assert_eq!(kind, EditKind::AddLink);
        assert!(!kind.needs_row_restore() && !kind.leaves_room());
        // Named from the command: the link isn't in the state before it's added.
        assert_eq!(
            toast_label(&f.state, &add, entity, kind).as_ref(),
            "Added link “https://docs.example.com/brief”"
        );
        // Adding it is an edit of its task, so an older target for the task goes stale and Ctrl+Z
        // undoes the addition, not something before it.
        assert!(touches(&add, task.into(), &f.state));
        let target = store.submit_undoable(add).expect("added");
        let added = store.view();
        assert!(added.is_live(entity));
        assert_eq!(
            kind.undone_label(&subject_title(&added, entity)).as_ref(),
            "Removed link “https://docs.example.com/brief”"
        );
        store.undo(&target).expect("undone");
        let after = store.view();
        assert!(!after.is_live(entity));
        assert!(kind.undo_changed_view(entity, &added, &after));
        assert!(!kind.undo_changed_view(entity, &after, &after));
    }

    #[test]
    fn a_removed_link_comes_back_with_undo() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let link = (f.state.links.values())
            .find(|l| l.task == task)
            .expect("a link")
            .id;
        let remove = crate::model::commands::delete_link(&f.state, link).expect("remove");
        let (entity, kind) = EditKind::of(&remove).expect("undoable");
        assert_eq!(kind, EditKind::DeleteLink);
        assert_eq!(
            toast_label(&f.state, &remove, entity, kind).as_ref(),
            "Removed link “Q4 roadmap: pre-read and agenda”"
        );
        // Removing a link is an edit of its task: an older target for the task goes stale.
        assert!(touches(&remove, task.into(), &f.state));
        let store = Store::fixture(&f);
        let target = store.submit_undoable(remove).expect("removed");
        // Its task deleted elsewhere, the restored link stays hidden, and the status bar says why.
        store
            .submit(Command::Delete { id: task.into() })
            .expect("the task deleted elsewhere");
        store.undo(&target).expect("exact inverse");
        let state = store.view();
        assert!(state.links[&link].deleted.is_none());
        assert_eq!(
            hidden_by_parent(kind, entity, &state).as_deref(),
            Some("Link restored, but its task was deleted elsewhere.")
        );
    }

    #[test]
    fn removing_an_attachment_uses_the_filename_and_restores_the_same_record() {
        use crate::store::Store;
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let file = f
            .state
            .attachments
            .values()
            .find(|a| a.task == task)
            .expect("attachment");
        let remove = todo_client::intent::delete(file.id);
        let (entity, kind) = EditKind::of(&remove).expect("undoable");
        assert_eq!(kind, EditKind::DeleteAttachment);
        assert_eq!(
            toast_label(&f.state, &remove, entity, kind).as_ref(),
            format!("Removed attachment “{}”", file.file_name)
        );
        assert!(touches(&remove, task.into(), &f.state));
        let store = Store::fixture(&f);
        let target = store.submit_undoable(remove).expect("removed");
        let removed = store.view();
        assert!(!removed.is_live(entity));
        store.undo(&target).expect("restored");
        assert!(kind.undo_changed_view(entity, &removed, &store.view()));
        assert_eq!(
            store.view().attachments[&file.id].storage_key,
            file.storage_key
        );
    }

    #[test]
    fn a_merge_undo_tracks_the_source_and_reverses_the_whole_batch() {
        use crate::store::Store;
        use todo_client::intent::merge;
        let f = fixtures::my_day();
        let source = f.ui.task.expect("selected task");
        let target = merge::candidates(&f.state, source, "").expect("picker")[0].id;
        let preview = merge::preview(&f.state, source, target).expect("confirmation");
        let cmd = merge::command(&f.state, source, target, &preview.question).expect("command");
        let (entity, kind) = EditKind::of(&cmd).expect("undoable");
        assert_eq!((entity, kind), (source.into(), EditKind::Merge));
        assert_eq!(
            toast_label(&f.state, &cmd, entity, kind).as_ref(),
            preview.toast
        );
        let store = Store::fixture(&f);
        let tracked = store.submit_undoable(cmd).expect("merged");
        let merged = store.view();
        assert!(!merged.is_live(entity));
        assert_eq!(merged.tasks[&source].merged_into, Some(target));
        store.undo(&tracked).expect("restored");
        let back = store.view();
        assert!(kind.undo_changed_view(entity, &merged, &back));
        assert!(back.tasks[&source].merged_into.is_none());
        assert_eq!(back.tasks[&target].notes, f.state.tasks[&target].notes);
        assert!(
            f.state
                .steps
                .values()
                .filter(|s| s.task == source)
                .all(|s| back.steps[&s.id].task == source)
        );
    }

    #[test]
    fn undoing_completion_delete_or_merge_can_restore_a_row() {
        assert!(EditKind::Complete.needs_row_restore());
        assert!(EditKind::Delete.needs_row_restore());
        assert!(EditKind::Merge.needs_row_restore());
        assert!(!EditKind::Move.needs_row_restore());
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
