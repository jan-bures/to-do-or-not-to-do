//! Plain words for the "couldn't sync" list ([`SyncClient::failed`](crate::SyncClient::failed)): what a
//! refused change was, what it was about, and why the server refused it. Never the server's own message,
//! which can be technical or quote task content. Pure, so every client words it the same way.

use std::borrow::Cow;
use todo_core::State;
use todo_core::command::Command;
use todo_core::ids::EntityId;

use crate::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};

/// Transfer status shared across native clients. No server message or URL reaches the UI.
pub const ATTACHMENT_OFFLINE: &str = "Connect to upload or download attachments.";
/// Uploads bypass the outbox, so a new task must reach the server first.
pub const ATTACHMENT_TASK_UNSYNCED: &str = "Wait for this task to sync, then add the file.";
pub const DOWNLOAD_TIMEOUT: &str = "The download timed out. Try again.";
#[must_use]
pub fn attachment_download(status: u16, code: Option<&str>) -> &'static str {
    match (status, code) {
        (404, Some("content_unavailable")) => "This attachment has no stored content.",
        (404, _) => "This attachment is no longer available.",
        (401, _) => "This device is signed out. Pair again to download attachments.",
        _ => "Couldn’t download the attachment.",
    }
}

/// Plain wording for a file upload failure. The only detail shown is a numeric size limit.
#[must_use]
pub fn attachment_upload(status: Option<u16>, max_bytes: Option<u64>) -> Cow<'static, str> {
    match status {
        Some(413) => {
            const MIB: u64 = 1024 * 1024;
            let Some(max) = max_bytes.filter(|&n| n > 0) else {
                return "That file is too large.".into();
            };
            let limit = if max.is_multiple_of(MIB) {
                format!("{} MiB", max / MIB)
            } else {
                format!("{max} bytes")
            };
            format!("That file is too large. The limit is {limit}.").into()
        }
        Some(507) => "Attachment storage is full.".into(),
        Some(429) => "Couldn’t upload right now. Try again later.".into(),
        _ => "Couldn’t upload the selected files.".into(),
    }
}

fn delete(id: EntityId) -> &'static str {
    match id {
        EntityId::Collection(_) => "Delete a collection",
        EntityId::List(_) => "Delete a list",
        EntityId::Task(_) => "Delete a task",
        EntityId::Step(_) => "Delete a step",
        EntityId::Tag(_) => "Delete a tag",
        EntityId::Link(_) => "Delete a link",
        EntityId::Attachment(_) => "Delete an attachment",
        EntityId::Reminder(_) => "Delete a reminder",
    }
}

fn restore(id: EntityId) -> &'static str {
    match id {
        EntityId::Collection(_) => "Restore a collection",
        EntityId::List(_) => "Restore a list",
        EntityId::Task(_) => "Restore a task",
        EntityId::Step(_) => "Restore a step",
        EntityId::Tag(_) => "Restore a tag",
        EntityId::Link(_) => "Restore a link",
        EntityId::Attachment(_) => "Restore an attachment",
        EntityId::Reminder(_) => "Restore a reminder",
    }
}

/// What the refused change was, in a few words: "Delete a task", "Add a list", "Undo a change".
#[must_use]
pub fn what(entry: &OutboxEntry) -> &'static str {
    let Some(command) = entry.command() else {
        return "Undo a change";
    };
    match command {
        Command::CreateCollection { .. } => "Add a collection",
        Command::UpdateCollection { .. } => "Change a collection",
        Command::CreateList { .. } => "Add a list",
        Command::UpdateList { .. } => "Change a list",
        Command::MoveList { .. } => "Move a list",
        Command::CreateTask(_) => "Add a task",
        Command::UpdateTask { .. } => "Edit a task",
        Command::MoveTask { .. } => "Move a task",
        Command::CompleteTask { .. } => "Complete a task",
        Command::UncompleteTask { .. } => "Reopen a task",
        Command::SetMyDay { .. } => "Change My Day",
        Command::AddTag { .. } => "Tag a task",
        Command::RemoveTag { .. } => "Untag a task",
        Command::MergeTasks { .. } => "Merge tasks",
        Command::AddNote { .. } => "Add to the notes",
        Command::CreateStep { .. } => "Add a step",
        Command::UpdateStep { .. } => "Rename a step",
        Command::CompleteStep { .. } => "Complete a step",
        Command::UncompleteStep { .. } => "Reopen a step",
        Command::CreateTag { .. } => "Add a tag",
        Command::UpdateTag { .. } => "Change a tag",
        Command::AddLink { .. } => "Add a link",
        Command::AddAttachment { .. } => "Add an attachment",
        Command::AddReminder { .. } => "Add a reminder",
        Command::UpdateReminder { .. } => "Change a reminder",
        Command::Reorder { id, .. } => match id {
            EntityId::Collection(_) => "Move a collection",
            EntityId::List(_) => "Move a list",
            EntityId::Task(_) => "Move a task",
            EntityId::Step(_) => "Move a step",
            _ => "Change the order",
        },
        Command::Delete { id } => delete(*id),
        Command::Restore { id } => restore(*id),
        Command::ImportEntity { .. } => "Import",
    }
}

/// The entity the change was about, if it names one that a state may hold.
#[must_use]
pub fn subject(entry: &OutboxEntry) -> Option<EntityId> {
    Some(match entry.command()? {
        Command::UpdateCollection { id, .. } => (*id).into(),
        Command::UpdateList { id, .. } | Command::MoveList { id, .. } => (*id).into(),
        Command::UpdateTask { id, .. }
        | Command::MoveTask { id, .. }
        | Command::CompleteTask { id }
        | Command::UncompleteTask { id }
        | Command::SetMyDay { id, .. } => (*id).into(),
        Command::AddTag { task, .. }
        | Command::RemoveTag { task, .. }
        | Command::AddNote { task, .. }
        | Command::CreateStep { task, .. }
        | Command::AddLink { task, .. }
        | Command::AddAttachment { task, .. }
        | Command::AddReminder { task, .. } => (*task).into(),
        Command::MergeTasks { source, .. } => (*source).into(),
        Command::UpdateStep { id, .. }
        | Command::CompleteStep { id }
        | Command::UncompleteStep { id } => (*id).into(),
        Command::CreateList { collection, .. } => (*collection)?.into(),
        Command::UpdateTag { id, .. } => (*id).into(),
        Command::Reorder { id, .. } | Command::Delete { id } | Command::Restore { id } => *id,
        Command::CreateCollection { .. }
        | Command::CreateTask(_)
        | Command::CreateTag { .. }
        | Command::UpdateReminder { .. }
        | Command::ImportEntity { .. } => return None,
    })
}

/// The name the change is about: what a new task, list, collection, step or tag was to be called, or
/// the current name of the entity it changed, as `state` knows it (deleted ones too). `None` when
/// there is none to show. Clients put it on one line before drawing it.
#[must_use]
pub fn name<'a>(entry: &'a OutboxEntry, state: &'a State) -> Option<&'a str> {
    let named = match entry.command()? {
        Command::CreateTask(task) => Some(task.title.as_str()),
        Command::CreateList { name, .. }
        | Command::CreateCollection { name, .. }
        | Command::CreateStep { title: name, .. }
        | Command::CreateTag { name, .. } => Some(name.as_str()),
        _ => None,
    };
    let name = named.or_else(|| match subject(entry)? {
        EntityId::Collection(id) => state.collections.get(&id).map(|c| c.name.as_str()),
        EntityId::List(id) => state.lists.get(&id).map(|l| l.name.as_str()),
        EntityId::Task(id) => state.tasks.get(&id).map(|t| t.title.as_str()),
        EntityId::Step(id) => state.steps.get(&id).map(|s| s.title.as_str()),
        EntityId::Tag(id) => state.tags.get(&id).map(|t| t.name.as_str()),
        EntityId::Link(_) | EntityId::Attachment(_) | EntityId::Reminder(_) => None,
    })?;
    (!name.trim().is_empty()).then_some(name)
}

/// Why the server refused it, in one short sentence: from the rejection's code, never its message.
#[must_use]
pub fn why(failed: &FailedEntry) -> &'static str {
    reason(
        &failed.rejection,
        matches!(failed.entry.op, Operation::Command { .. }),
    )
}

/// A short undo refusal for a toast or an activity row, based only on the machine-readable code.
#[must_use]
pub fn undo_message(code: &str) -> &'static str {
    match code {
        "duplicate_name" => "Couldn't undo: another tag already uses that name.",
        "undo_conflict" => "Couldn't undo: the item was changed again.",
        "undo_no_event" | "event_not_found" => {
            "Couldn't undo: the original change couldn't be found."
        }
        "epoch_mismatch" | "event_mismatch" => "Couldn't undo: the server's history changed.",
        "not_found" => "Couldn't undo: the item no longer exists.",
        "already_undone" => "Couldn't undo: the change was already undone.",
        "undo_invariant" => "Couldn't undo: something the change depends on changed.",
        _ => "Couldn't undo that.",
    }
}

fn reason(rejection: &Rejection, command: bool) -> &'static str {
    match rejection.code.as_str() {
        "deleted" => "It was deleted elsewhere.",
        "not_found" => "It no longer exists.",
        "already_deleted" => "It was already deleted.",
        "not_deleted" => "It wasn't deleted.",
        "already_exists" => "It already exists.",
        "duplicate_name" => "That name is already taken.",
        "move_source_changed" => "The task was moved or merged elsewhere.",
        "forbidden" | "not_own_event" => "This device isn't allowed to do that.",
        "undo_conflict" => "It was changed again since.",
        "undo_no_event" | "event_not_found" => "The original change couldn't be found.",
        "epoch_mismatch" | "event_mismatch" => "The server's history changed.",
        "already_undone" => "It was already undone.",
        "undo_invariant" => "Something it depends on changed.",
        "invalid" | "invalid_recurrence" | "invalid_position" | "bad_placement" | "same_entity" => {
            "The server didn't accept it."
        }
        _ if command => "The server refused it.",
        _ => "The server couldn't undo it.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::command::{NewTask, Placement};
    use todo_core::ids::{ListId, StepId, TaskId};
    use uuid::Uuid;

    fn entry(op: Operation) -> OutboxEntry {
        OutboxEntry {
            command_id: Uuid::from_u128(1),
            op,
            submitted_at: todo_core::time::Timestamp::UNIX_EPOCH,
            attempts: 1,
            acked: None,
            acked_epoch: None,
        }
    }

    fn command(command: Command) -> OutboxEntry {
        entry(Operation::Command { command })
    }

    fn failed(entry: OutboxEntry, code: &str) -> FailedEntry {
        FailedEntry {
            entry,
            failed_at: todo_core::time::Timestamp::UNIX_EPOCH,
            rejection: Rejection {
                status: 409,
                code: code.into(),
                message: "raw: seq 41 conflicts with \u{201c}Secret title\u{201d}".into(),
            },
        }
    }

    #[test]
    fn undo_refusals_use_fixed_words_and_fall_back_for_unknown_codes() {
        for (code, message) in [
            (
                "undo_conflict",
                "Couldn't undo: the item was changed again.",
            ),
            (
                "undo_no_event",
                "Couldn't undo: the original change couldn't be found.",
            ),
            (
                "event_not_found",
                "Couldn't undo: the original change couldn't be found.",
            ),
            (
                "epoch_mismatch",
                "Couldn't undo: the server's history changed.",
            ),
            (
                "event_mismatch",
                "Couldn't undo: the server's history changed.",
            ),
            ("not_found", "Couldn't undo: the item no longer exists."),
            (
                "already_undone",
                "Couldn't undo: the change was already undone.",
            ),
            (
                "undo_invariant",
                "Couldn't undo: something the change depends on changed.",
            ),
            ("an_unknown_code", "Couldn't undo that."),
        ] {
            assert_eq!(undo_message(code), message, "{code}");
        }
    }

    #[test]
    fn upload_refusals_use_fixed_words_and_an_optional_size_limit() {
        assert_eq!(
            attachment_upload(Some(413), None),
            "That file is too large."
        );
        assert_eq!(
            attachment_upload(Some(413), Some(7)),
            "That file is too large. The limit is 7 bytes."
        );
        assert_eq!(
            attachment_upload(Some(413), Some(25 * 1024 * 1024)),
            "That file is too large. The limit is 25 MiB."
        );
        assert_eq!(
            attachment_upload(Some(507), None),
            "Attachment storage is full."
        );
        assert_eq!(
            attachment_upload(Some(429), None),
            "Couldn’t upload right now. Try again later."
        );
        assert_eq!(
            attachment_upload(Some(502), None),
            attachment_upload(None, None)
        );
    }

    #[test]
    fn a_refusal_is_told_in_plain_words_never_the_servers() {
        let task = TaskId::from_uuid(Uuid::from_u128(2));
        let delete = command(Command::Delete { id: task.into() });
        assert_eq!(what(&delete), "Delete a task");
        for code in [
            "deleted",
            "not_found",
            "already_deleted",
            "not_deleted",
            "already_exists",
            "duplicate_name",
            "forbidden",
            "invalid",
            "same_entity",
            "undo_conflict",
            "something_new",
        ] {
            let why = why(&failed(delete.clone(), code));
            assert!(why.ends_with('.'), "{code}: {why}");
            assert!(!why.contains("seq") && !why.contains("Secret"), "{code}");
        }
        assert_eq!(
            why(&failed(delete.clone(), "deleted")),
            "It was deleted elsewhere."
        );
        assert_eq!(
            why(&failed(delete, "something_new")),
            "The server refused it."
        );
        // An undo says so, and an unknown refusal of it reads as one.
        let undo = entry(Operation::UndoAfter {
            original: Uuid::from_u128(3),
        });
        assert_eq!(what(&undo), "Undo a change");
        assert_eq!(subject(&undo), None);
        assert_eq!(
            why(&failed(undo.clone(), "undo_conflict")),
            "It was changed again since."
        );
        assert_eq!(why(&failed(undo, "teapot")), "The server couldn't undo it.");
    }

    #[test]
    fn a_stale_move_explains_what_changed() {
        let moved = command(Command::MoveTask {
            id: TaskId::from_uuid(Uuid::from_u128(5)),
            list: ListId::from_uuid(Uuid::from_u128(6)),
            from_list: Some(ListId::from_uuid(Uuid::from_u128(4))),
            placement: Placement::First,
        });
        assert_eq!(
            why(&failed(moved, "move_source_changed")),
            "The task was moved or merged elsewhere."
        );
    }

    #[test]
    fn the_name_comes_from_the_command_or_the_state() {
        let mut state = State::new();
        let list = ListId::from_uuid(Uuid::from_u128(4));
        let task = TaskId::from_uuid(Uuid::from_u128(5));
        // A new task names itself, even though the state never had it.
        let add = command(Command::CreateTask(Box::new(NewTask::new(
            task,
            list,
            "Buy stamps",
        ))));
        assert_eq!(what(&add), "Add a task");
        assert_eq!(name(&add, &state), Some("Buy stamps"));
        // A change to something the state doesn't hold has no name to show.
        let moved = command(Command::MoveTask {
            id: task,
            list,
            from_list: None,
            placement: Placement::First,
        });
        assert_eq!(what(&moved), "Move a task");
        assert_eq!(subject(&moved), Some(task.into()));
        assert_eq!(name(&moved, &state), None);
        // Once the state has it, its title.
        let actor = todo_core::Actor::owner("test").expect("actor");
        let at = "2026-11-21T10:00:00Z".parse().expect("time");
        for cmd in [
            crate::intent::create_list(list, None, "Errands"),
            Command::CreateTask(Box::new(NewTask::new(task, list, "Buy stamps"))),
        ] {
            for e in todo_core::decide(&state, &actor, &cmd, at).expect("valid") {
                todo_core::apply(&mut state, &e).expect("applies");
            }
        }
        assert_eq!(name(&moved, &state), Some("Buy stamps"));
        let rename = command(Command::UpdateList {
            id: list,
            name: Some("Shops".into()),
            color: None,
        });
        assert_eq!(name(&rename, &state), Some("Errands"));
        let step = command(Command::UpdateStep {
            id: StepId::from_uuid(Uuid::from_u128(6)),
            title: "x".into(),
        });
        assert_eq!(what(&step), "Rename a step");
        assert_eq!(name(&step, &state), None);
    }
}
