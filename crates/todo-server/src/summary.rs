//! One-line, human-readable summaries of commands and events for responses and the activity feed.
//!
//! Summaries contain task titles, so they are returned to authenticated clients only and never logged.

use todo_core::command::Command;
use todo_core::event::{Event, EventPayload, FieldChange};
use todo_core::ids::EntityId;
use todo_core::model::{Entity, LinkKind};
use todo_core::state::State;

const MAX_LABEL: usize = 60;

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= MAX_LABEL {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(MAX_LABEL - 1).collect();
        format!("{cut}…")
    }
}

fn kind(id: EntityId) -> &'static str {
    match id {
        EntityId::Collection(_) => "collection",
        EntityId::List(_) => "list",
        EntityId::Task(_) => "task",
        EntityId::Step(_) => "step",
        EntityId::Tag(_) => "tag",
        EntityId::Link(_) => "link",
        EntityId::Attachment(_) => "attachment",
        EntityId::Reminder(_) => "reminder",
    }
}

fn entity_label(e: &Entity) -> Option<String> {
    Some(match e {
        Entity::Collection(c) => clip(&c.name),
        Entity::List(l) => clip(&l.name),
        Entity::Task(t) => clip(&t.title),
        Entity::Step(s) => clip(&s.title),
        Entity::Tag(t) => clip(&t.name),
        Entity::Link(l) => match &l.kind {
            LinkKind::Web { url, title } => clip(title.as_deref().unwrap_or(url)),
            LinkKind::Email(m) => clip(&m.subject),
        },
        Entity::Attachment(a) => clip(&a.file_name),
        Entity::Reminder(_) => return None,
    })
}

/// `task "Buy milk"`, or just `task` when the entity is gone.
fn describe(state: &State, id: EntityId) -> String {
    match state.entity(id).as_ref().and_then(entity_label) {
        Some(label) => format!("{} \"{label}\"", kind(id)),
        None => kind(id).to_owned(),
    }
}

fn field_name(c: &FieldChange) -> Option<&'static str> {
    Some(match c {
        FieldChange::Name(_) => "name",
        FieldChange::Color(_) => "color",
        FieldChange::Position(_) => "position",
        FieldChange::Title(_) => "title",
        FieldChange::Notes(_) | FieldChange::NoteAdded(_) => "notes",
        FieldChange::Due(_) => "due date",
        FieldChange::StartDate(_) => "start date",
        FieldChange::Status(_) => "status",
        FieldChange::ListFeatures(_) => "list settings",
        FieldChange::Priority(_) => "priority",
        FieldChange::MyDay(_) => "My Day",
        FieldChange::CompletedAt(_) => "completion",
        FieldChange::Recurrence(_) => "recurrence",
        FieldChange::ListCollection(_) => "collection",
        FieldChange::TaskList(_) => "list",
        FieldChange::ParentTask(_) => "task",
        FieldChange::MergedInto(_) => "merge",
        FieldChange::ReminderAt(_) => "time",
        FieldChange::TagAdded { .. } | FieldChange::TagRemoved { .. } => "tags",
        FieldChange::Touch(_) => return None,
    })
}

/// Summary of one stored event, described against the current state.
#[must_use]
pub fn event(state: &State, event: &Event) -> String {
    let target = event.payload.target();
    let what = match &event.payload {
        EventPayload::Created { entity } => match entity_label(entity) {
            Some(l) => format!("Created {} \"{l}\"", kind(target)),
            None => format!("Created {}", kind(target)),
        },
        EventPayload::Withdrawn { entity } => match entity_label(entity) {
            Some(l) => format!("Withdrew {} \"{l}\"", kind(target)),
            None => format!("Withdrew {}", kind(target)),
        },
        EventPayload::Deleted { .. } => format!("Deleted {}", describe(state, target)),
        EventPayload::Restored { .. } => format!("Restored {}", describe(state, target)),
        EventPayload::Updated { changes, .. } => {
            let mut fields: Vec<&str> = changes.iter().filter_map(field_name).collect();
            fields.dedup();
            let completed = changes.iter().find_map(|c| match c {
                FieldChange::CompletedAt(c) => Some(c.new.is_some()),
                _ => None,
            });
            match (completed, fields.as_slice()) {
                (Some(true), ["completion"]) => format!("Completed {}", describe(state, target)),
                (Some(false), ["completion"]) => format!("Reopened {}", describe(state, target)),
                (_, []) => format!("Updated {}", describe(state, target)),
                _ => format!(
                    "Updated {} ({})",
                    describe(state, target),
                    fields.join(", ")
                ),
            }
        }
    };
    if event.undoes.is_some() {
        format!("Undo: {what}")
    } else {
        what
    }
}

/// Summary of a handled command, described against the state after its events were applied.
#[must_use]
pub fn command(state: &State, cmd: &Command, events: &[Event]) -> String {
    use Command as C;
    if events.is_empty() {
        return "No changes".to_owned();
    }
    let d = |id: EntityId| describe(state, id);
    let main = match cmd {
        C::CreateCollection { id, .. } => format!("Created {}", d((*id).into())),
        C::CreateList { id, .. } => format!("Created {}", d((*id).into())),
        C::CreateTask(n) => format!(
            "Created {} in {}",
            d(n.id.into()),
            d(EntityId::List(n.list))
        ),
        C::CreateStep { id, task, .. } => {
            format!("Added {} to {}", d((*id).into()), d((*task).into()))
        }
        C::CreateTag { id, .. } => format!("Created {}", d((*id).into())),
        C::MoveList { id, .. } => format!("Moved {}", d((*id).into())),
        C::MoveTask { id, list, .. } => {
            format!("Moved {} to {}", d((*id).into()), d((*list).into()))
        }
        C::CompleteTask { id } => format!("Completed {}", d((*id).into())),
        C::UncompleteTask { id } => format!("Reopened {}", d((*id).into())),
        C::SetMyDay { id, date } => match date {
            Some(_) => format!("Added {} to My Day", d((*id).into())),
            None => format!("Removed {} from My Day", d((*id).into())),
        },
        C::AddTag { task, tag } => {
            format!("Tagged {} with {}", d((*task).into()), d((*tag).into()))
        }
        C::RemoveTag { task, tag } => {
            format!("Removed {} from {}", d((*tag).into()), d((*task).into()))
        }
        C::BulkTasks { .. } => "Updated selected tasks".to_owned(),
        C::MergeTasks { source, target } => {
            format!(
                "Merged {} into {}",
                d((*source).into()),
                d((*target).into())
            )
        }
        C::AddNote { task, .. } => format!("Added a note to {}", d((*task).into())),
        C::CompleteStep { id } => format!("Completed {}", d((*id).into())),
        C::UncompleteStep { id } => format!("Reopened {}", d((*id).into())),
        C::AddLink { id, task, .. } => {
            format!("Linked {} to {}", d((*id).into()), d((*task).into()))
        }
        C::AddAttachment { id, task, .. } => {
            format!("Attached {} to {}", d((*id).into()), d((*task).into()))
        }
        C::AddReminder { task, .. } => format!("Added a reminder to {}", d((*task).into())),
        C::ImportEntity { entity } => format!("Imported {}", d(entity.id())),
        C::Reorder { id, .. } => format!("Reordered {}", d(*id)),
        C::Delete { id } => format!("Deleted {}", d(*id)),
        C::Restore { id } => format!("Restored {}", d(*id)),
        C::UpdateCollection { .. }
        | C::UpdateList { .. }
        | C::UpdateTask { .. }
        | C::MoveTaskStatus { .. }
        | C::UpdateStep { .. }
        | C::UpdateTag { .. }
        | C::UpdateReminder { .. } => event(state, &events[0]),
    };
    match events.len() {
        1 => main,
        n => format!(
            "{main} (+{} more change{})",
            n - 1,
            if n == 2 { "" } else { "s" }
        ),
    }
}

/// Summary of an undo (or redo) that produced `events`.
#[must_use]
pub fn undo(state: &State, events: &[Event]) -> String {
    match events {
        [] => "No changes".to_owned(),
        [one] => event(state, one),
        [first, rest @ ..] => format!(
            "{} (+{} more change{})",
            event(state, first),
            rest.len(),
            if rest.len() == 1 { "" } else { "s" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::actor::Actor;
    use todo_core::event::Change;
    use todo_core::ids::{EventId, TaskId};
    use todo_core::model::Stamp;
    use todo_core::time::Timestamp;
    use uuid::Uuid;

    #[test]
    fn touch_only_updates_have_no_empty_field_list() {
        let id = EventId::from_uuid(Uuid::from_u128(1));
        let at = Timestamp::UNIX_EPOCH;
        let stamp = Stamp {
            at,
            by: Actor::System,
        };
        let update = Event {
            id,
            seq: Some(1),
            actor: Actor::System,
            at,
            group: id,
            undoes: None,
            forced: false,
            payload: EventPayload::Updated {
                target: TaskId::from_uuid(Uuid::from_u128(2)).into(),
                changes: vec![FieldChange::Touch(Change::new(stamp.clone(), stamp))],
            },
        };
        assert_eq!(event(&State::new(), &update), "Updated task");
    }

    #[test]
    fn long_labels_are_clipped() {
        let long = "x".repeat(200);
        assert_eq!(clip(&long).chars().count(), 60);
        assert_eq!(clip("  short "), "short");
    }
}
