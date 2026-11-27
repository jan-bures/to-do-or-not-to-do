//! Command validation: turns a [`Command`] into events.
//!
//! [`decide`] is pure. It checks permissions, validates the command against the current state, resolves
//! positions, and returns the events to store; it does not change the state (use
//! [`apply`](crate::apply::apply) for that). Event IDs are derived deterministically from the actor, the
//! command and the time, so retrying the same command at the same time yields the same event IDs.

use crate::access::authorize;
use crate::actor::Actor;
use crate::apply::apply;
use crate::command::{BulkTaskAction, Command, NewTask, Placement, TaskPatch};
use crate::error::DomainError;
use crate::event::{Change, Event, EventPayload, FieldChange};
use crate::ids::{EntityId, EventId, ListId, ReminderId, StepId, TagId, TaskId};
use crate::model::{
    AttachmentRef, Collection, Entity, Link, LinkKind, List, ListView, Reminder, Stamp, Step, Tag,
    Task, TaskSort, Tombstone, WorkflowStatus,
};
use crate::state::{Scope, State};
use crate::time::{Date, Timestamp, add_days, days_between, shift_days};

/// Longest accepted task or step title, in characters.
pub const MAX_TITLE: usize = 500;
/// Longest accepted name of a collection, list or tag, in characters.
pub const MAX_NAME: usize = 200;
/// Longest accepted notes, in bytes.
pub const MAX_NOTES: usize = 100_000;
/// Longest accepted colour string.
pub const MAX_COLOR: usize = 32;
/// Longest accepted URL, storage key or similar short string.
pub const MAX_URL: usize = 2048;
/// Most tags a new task may be created with (more can be added one at a time).
pub const MAX_TASK_TAGS: usize = 64;
/// Upper bound on one bulk command, including requests from outside the app.
pub const MAX_BULK_TASKS: usize = 1000;

/// Checks the limits a command must meet whatever the state and the actor: how many entities it may
/// list. Pure and cheap, and [`decide`] runs it first, so a server can run it before anything else
/// (before any lookup or lock) and give the same answer.
pub fn check_limits(command: &Command) -> Result<(), DomainError> {
    match command {
        Command::CreateTask(n) if n.tags.len() > MAX_TASK_TAGS => Err(DomainError::invalid(
            "tags",
            format!("more than {MAX_TASK_TAGS} tags"),
        )),
        Command::BulkTasks { tasks, .. } if tasks.is_empty() || tasks.len() > MAX_BULK_TASKS => {
            Err(DomainError::invalid(
                "tasks",
                format!("expected 1 to {MAX_BULK_TASKS} tasks"),
            ))
        }
        Command::BulkTasks { tasks, .. }
            if tasks
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != tasks.len() =>
        {
            Err(DomainError::invalid("tasks", "duplicate task"))
        }
        _ => Ok(()),
    }
}

struct Decider<'a> {
    state: &'a State,
    actor: &'a Actor,
    now: Timestamp,
    seed: String,
    events: Vec<Event>,
}

fn text(field: &'static str, s: &str, max: usize) -> Result<String, DomainError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(DomainError::invalid(field, "must not be empty"));
    }
    if s.chars().count() > max {
        return Err(DomainError::invalid(
            field,
            format!("longer than {max} characters"),
        ));
    }
    Ok(s.to_owned())
}

fn color(c: &Option<String>) -> Result<(), DomainError> {
    match c {
        Some(c) if c.is_empty() || c.len() > MAX_COLOR => {
            Err(DomainError::invalid("color", "empty or too long"))
        }
        _ => Ok(()),
    }
}

fn notes(s: &str) -> Result<(), DomainError> {
    if s.len() > MAX_NOTES {
        Err(DomainError::invalid("notes", "too long"))
    } else {
        Ok(())
    }
}

fn short(field: &'static str, s: &str) -> Result<(), DomainError> {
    if s.trim().is_empty() || s.len() > MAX_URL {
        Err(DomainError::invalid(field, "empty or too long"))
    } else {
        Ok(())
    }
}

fn web_url(s: &str) -> Result<(), DomainError> {
    short("url", s)?;
    let ok = ["http://", "https://"].iter().any(|p| s.starts_with(p));
    if ok && !s.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err(DomainError::invalid("url", "must be an http or https URL"))
    }
}

/// Checks a link the way [`decide`] checks `Command::AddLink`: a web link needs an `http` or `https` URL
/// without whitespace, at most [`MAX_URL`] bytes, and an email link needs a sender and a message ID.
/// Clients use it to say what's wrong before they submit.
pub fn validate_link(kind: &LinkKind) -> Result<(), DomainError> {
    match kind {
        LinkKind::Web { url, title } => {
            web_url(url)?;
            if let Some(t) = title {
                short("title", t)?;
            }
        }
        LinkKind::Email(e) => {
            // An email may have an empty subject; only reject over-long ones.
            if e.subject.len() > MAX_URL {
                return Err(DomainError::invalid("subject", "too long"));
            }
            short("sender", &e.sender)?;
            short("message_id", &e.message_id)?;
            if let Some(w) = &e.web_link {
                web_url(w)?;
            }
        }
    }
    Ok(())
}

fn stored_text(field: &'static str, s: &str, max: usize) -> Result<(), DomainError> {
    if s.trim().is_empty() {
        Err(DomainError::invalid(field, "must not be empty"))
    } else if s.chars().count() > max {
        Err(DomainError::invalid(
            field,
            format!("longer than {max} characters"),
        ))
    } else {
        Ok(())
    }
}

fn validate_span(start: Option<Date>, due: Option<Date>) -> Result<(), DomainError> {
    if start.zip(due).is_some_and(|(start, due)| start > due) {
        return Err(DomainError::invalid(
            "start_date",
            "must be on or before due date",
        ));
    }
    Ok(())
}

/// Checks the invariants that [`decide`] enforces on an entity value. Used by
/// [`undo`](crate::undo::undo) to refuse
/// compensations that would leave an invalid entity behind.
pub fn validate_entity(entity: &Entity) -> Result<(), DomainError> {
    match entity {
        Entity::List(l) => {
            stored_text("name", &l.name, MAX_NAME)?;
            color(&l.color)?;
            if !l.features.allows(l.features.default_view) {
                return Err(DomainError::invalid(
                    "default_view",
                    "requires its feature to be enabled",
                ));
            }
            Ok(())
        }
        Entity::Collection(Collection { name, color: c, .. })
        | Entity::Tag(Tag { name, color: c, .. }) => {
            stored_text("name", name, MAX_NAME)?;
            color(c)
        }
        Entity::Task(t) => {
            stored_text("title", &t.title, MAX_TITLE)?;
            notes(&t.notes)?;
            validate_span(t.start_date, t.due)?;
            if let Some(r) = &t.recurrence {
                r.validate()?;
                if t.due.is_none() {
                    return Err(DomainError::invalid("recurrence", "needs a due date"));
                }
            }
            Ok(())
        }
        Entity::Step(s) => stored_text("title", &s.title, MAX_TITLE),
        Entity::Link(l) => validate_link(&l.kind),
        Entity::Attachment(a) => {
            short("file_name", &a.file_name)?;
            if a.file_name.contains(['/', '\\']) {
                return Err(DomainError::invalid("file_name", "must not contain a path"));
            }
            short("media_type", &a.media_type)?;
            short("storage_key", &a.storage_key)
        }
        Entity::Reminder(_) => Ok(()),
    }
}

/// All writes that make a tag name visible use the same case-insensitive comparison.
pub(crate) fn unique_tag_name(
    state: &State,
    name: &str,
    except: Option<TagId>,
) -> Result<(), DomainError> {
    let lower = name.to_lowercase();
    if state
        .tags
        .values()
        .any(|t| t.deleted.is_none() && Some(t.id) != except && t.name.to_lowercase() == lower)
    {
        Err(DomainError::DuplicateName(name.to_owned()))
    } else {
        Ok(())
    }
}

impl<'a> Decider<'a> {
    fn stamp(&self) -> Stamp {
        Stamp {
            at: self.now,
            by: self.actor.clone(),
        }
    }

    fn push(&mut self, payload: EventPayload) {
        let idx = self.events.len();
        let id = EventId::derive(self.now, format!("{}|{idx}", self.seed).as_bytes());
        self.events.push(Event {
            id,
            seq: None,
            actor: self.actor.clone(),
            at: self.now,
            group: id,
            undoes: None,
            forced: false,
            payload,
        });
    }

    fn derived<T>(&self, tag: &str, f: impl FnOnce(Timestamp, &[u8]) -> T) -> T {
        f(self.now, format!("{}|{tag}", self.seed).as_bytes())
    }

    /// IDs are never reused: not while the entity exists, and not after its creation was undone either,
    /// because events of the old entity (and their undo/redo chains) still refer to it.
    fn require_new(&self, id: EntityId) -> Result<(), DomainError> {
        if self.state.exists(id) || self.state.journal.id_used(id) {
            Err(DomainError::AlreadyExists(id))
        } else {
            Ok(())
        }
    }

    fn require_live(&self, id: EntityId) -> Result<(), DomainError> {
        if !self.state.exists(id) {
            Err(DomainError::NotFound(id))
        } else if !self.state.is_live(id) {
            Err(DomainError::Deleted(id))
        } else {
            Ok(())
        }
    }

    fn task(&self, id: TaskId) -> Result<&'a Task, DomainError> {
        self.require_live(id.into())?;
        self.state
            .tasks
            .get(&id)
            .ok_or(DomainError::NotFound(id.into()))
    }

    /// Emits an update of a task, adding the last-modified bookkeeping.
    fn update_task(&mut self, task: &Task, mut changes: Vec<FieldChange>) {
        if changes.is_empty() {
            return;
        }
        changes.push(FieldChange::Touch(Change::new(
            task.updated.clone(),
            self.stamp(),
        )));
        self.push(EventPayload::Updated {
            target: task.id.into(),
            changes,
        });
    }

    fn update(&mut self, target: EntityId, changes: Vec<FieldChange>) {
        if !changes.is_empty() {
            self.push(EventPayload::Updated { target, changes });
        }
    }

    fn name_changes(
        &self,
        cur_name: &str,
        cur_color: &Option<String>,
        name: &Option<String>,
        new_color: &Option<Option<String>>,
        limit: usize,
    ) -> Result<Vec<FieldChange>, DomainError> {
        let mut ch = Vec::new();
        if let Some(n) = name {
            let n = text("name", n, limit)?;
            if n != cur_name {
                ch.push(FieldChange::Name(Change::new(cur_name.to_owned(), n)));
            }
        }
        if let Some(c) = new_color {
            color(c)?;
            if c != cur_color {
                ch.push(FieldChange::Color(Change::new(
                    cur_color.clone(),
                    c.clone(),
                )));
            }
        }
        Ok(ch)
    }

    fn create_task(&mut self, n: &NewTask) -> Result<(), DomainError> {
        self.require_new(n.id.into())?;
        self.require_live(n.list.into())?;
        let title = text("title", &n.title, MAX_TITLE)?;
        notes(&n.notes)?;
        validate_span(n.start_date, n.due)?;
        if let Some(r) = &n.recurrence {
            r.validate()?;
            if n.due.is_none() {
                return Err(DomainError::invalid("recurrence", "needs a due date"));
            }
        }
        for t in &n.tags {
            self.require_live((*t).into())?;
        }
        let position = self
            .state
            .resolve_placement(Scope::Tasks(n.list), None, &n.placement)?;
        let stamp = self.stamp();
        self.push(EventPayload::Created {
            entity: Entity::Task(Task {
                id: n.id,
                list: n.list,
                title,
                notes: n.notes.clone(),
                due: n.due,
                status: n.status,
                start_date: n.start_date,
                priority: n.priority,
                my_day: n.my_day,
                completed_at: None,
                recurrence: n.recurrence.clone(),
                tags: n.tags.iter().copied().collect(),
                position,
                created: stamp.clone(),
                updated: stamp,
                deleted: None,
                merged_into: None,
            }),
        });
        Ok(())
    }

    fn update_task_fields(&mut self, id: TaskId, p: &TaskPatch) -> Result<(), DomainError> {
        let t = self.task(id)?;
        let mut ch = Vec::new();
        if let Some(v) = &p.title {
            let v = text("title", v, MAX_TITLE)?;
            if v != t.title {
                ch.push(FieldChange::Title(Change::new(t.title.clone(), v)));
            }
        }
        if let Some(v) = &p.notes {
            notes(v)?;
            if *v != t.notes {
                ch.push(FieldChange::Notes(Change::new(t.notes.clone(), v.clone())));
            }
        }
        let mut due = p.due.unwrap_or(t.due);
        let mut start_date = p.start_date.unwrap_or(t.start_date);
        // Honor a one-endpoint edit against the current span, including writes from older clients
        // that only know about due dates. If both endpoints are explicit, validate them as given.
        if start_date.zip(due).is_some_and(|(start, end)| start > end) {
            match (p.start_date, p.due) {
                (None, Some(_)) => start_date = due,
                (Some(_), None) => due = start_date,
                _ => validate_span(start_date, due)?,
            }
        }
        if start_date != t.start_date {
            ch.push(FieldChange::StartDate(Change::new(
                t.start_date,
                start_date,
            )));
        }
        if let Some(status) = p.status.filter(|s| *s != t.status) {
            ch.push(FieldChange::Status(Change::new(t.status, status)));
        }
        let rec = p.recurrence.clone().unwrap_or_else(|| t.recurrence.clone());
        if let Some(r) = &rec {
            r.validate()?;
            if due.is_none() {
                return Err(DomainError::invalid("recurrence", "needs a due date"));
            }
        }
        if due != t.due {
            ch.push(FieldChange::Due(Change::new(t.due, due)));
        }
        if let Some(v) = p.priority
            && v != t.priority
        {
            ch.push(FieldChange::Priority(Change::new(t.priority, v)));
        }
        if rec != t.recurrence {
            ch.push(FieldChange::Recurrence(Change::new(
                t.recurrence.clone(),
                rec,
            )));
        }
        self.update_task(t, ch);
        Ok(())
    }

    fn move_task(
        &mut self,
        id: TaskId,
        list: crate::ids::ListId,
        from_list: Option<crate::ids::ListId>,
        pl: &Placement,
    ) -> Result<(), DomainError> {
        let t = self
            .state
            .tasks
            .get(&id)
            .ok_or(DomainError::NotFound(id.into()))?;
        // A merge also tombstones the task. Report the failed precondition before checking liveness.
        if from_list.is_some_and(|from| from != t.list || t.merged_into.is_some()) {
            return Err(DomainError::MoveSourceChanged);
        }
        self.require_live(id.into())?;
        self.require_live(list.into())?;
        let pos = self
            .state
            .resolve_placement(Scope::Tasks(list), Some(id.into()), pl)?;
        let mut ch = Vec::new();
        let other_list = list != t.list;
        if other_list {
            ch.push(FieldChange::TaskList(Change::new(t.list, list)));
        }
        // A move to another list always records the position, even a key that stays the same: then a
        // later reorder in the new list makes undoing the move a conflict, instead of taking the task
        // back with the key it got there (in the wrong place among its old siblings). Moves recorded
        // before this (the list only) still apply and undo as they did.
        if other_list || pos != t.position {
            ch.push(FieldChange::Position(Change::new(t.position.clone(), pos)));
        }
        self.update_task(t, ch);
        Ok(())
    }

    fn move_task_status(
        &mut self,
        id: TaskId,
        list: ListId,
        status: WorkflowStatus,
        placement: Placement,
    ) -> Result<(), DomainError> {
        let t = self.task(id)?;
        if t.list != list {
            return Err(DomainError::MoveSourceChanged);
        }
        if let Placement::Before(anchor) | Placement::After(anchor) = placement
            && !matches!(anchor, EntityId::Task(other)
                if self.state.tasks.get(&other).is_some_and(|s| !s.is_done() && s.status == status))
        {
            return Err(DomainError::invalid(
                "placement",
                "card is not in the destination column",
            ));
        }
        let pos = self
            .state
            .resolve_placement(Scope::Tasks(list), Some(id.into()), &placement)?;
        let mut changes = Vec::new();
        if t.is_done() {
            changes.push(FieldChange::CompletedAt(Change::new(t.completed_at, None)));
        }
        if t.status != status {
            changes.push(FieldChange::Status(Change::new(t.status, status)));
        }
        if t.position != pos {
            changes.push(FieldChange::Position(Change::new(t.position.clone(), pos)));
        }
        self.update_task(t, changes);
        Ok(())
    }

    fn set_completed(&mut self, id: TaskId, done: bool) -> Result<(), DomainError> {
        let t = self.task(id)?;
        if t.is_done() == done {
            return Ok(());
        }
        let new = done.then_some(self.now);
        self.update_task(
            t,
            vec![FieldChange::CompletedAt(Change::new(t.completed_at, new))],
        );
        if done {
            self.spawn_next(t)?;
        }
        Ok(())
    }

    /// Creates the next occurrence of a recurring task that was just completed.
    fn spawn_next(&mut self, t: &Task) -> Result<(), DomainError> {
        let (Some(rec), Some(due)) = (&t.recurrence, t.due) else {
            return Ok(());
        };
        let Some(next) = rec.next_after(due) else {
            return Ok(());
        };
        let delta = days_between(due, next);
        let new_id: TaskId = self.derived("spawn", TaskId::derive);
        let position = self.state.resolve_placement(
            Scope::Tasks(t.list),
            None,
            &Placement::After(t.id.into()),
        )?;
        let stamp = self.stamp();
        self.push(EventPayload::Created {
            entity: Entity::Task(Task {
                id: new_id,
                list: t.list,
                title: t.title.clone(),
                notes: t.notes.clone(),
                due: Some(next),
                status: WorkflowStatus::Todo,
                start_date: t.start_date.and_then(|start| add_days(start, delta)),
                priority: t.priority,
                my_day: None,
                completed_at: None,
                recurrence: t.recurrence.clone(),
                tags: t.tags.clone(),
                position,
                created: stamp.clone(),
                updated: stamp.clone(),
                deleted: None,
                merged_into: None,
            }),
        });
        for (i, (_, sid)) in self
            .state
            .siblings(Scope::Steps(t.id))
            .into_iter()
            .enumerate()
        {
            let EntityId::Step(sid) = sid else { continue };
            let Some(s) = self.state.steps.get(&sid) else {
                continue;
            };
            let copy_id: StepId = self.derived(&format!("spawn-step-{i}"), StepId::derive);
            self.push(EventPayload::Created {
                entity: Entity::Step(Step {
                    id: copy_id,
                    task: new_id,
                    title: s.title.clone(),
                    completed_at: None,
                    position: s.position.clone(),
                    created: stamp.clone(),
                    deleted: None,
                }),
            });
        }
        let mut reminders: Vec<_> = self
            .state
            .reminders
            .values()
            .filter(|r| r.task == t.id && r.deleted.is_none())
            .collect();
        reminders.sort_by_key(|r| (r.at, r.id));
        for (i, r) in reminders.into_iter().enumerate() {
            let Some(at) = shift_days(r.at, delta) else {
                continue;
            };
            let copy_id: ReminderId = self.derived(&format!("spawn-rem-{i}"), ReminderId::derive);
            self.push(EventPayload::Created {
                entity: Entity::Reminder(Reminder {
                    id: copy_id,
                    task: new_id,
                    at,
                    created: stamp.clone(),
                    deleted: None,
                }),
            });
        }
        Ok(())
    }

    fn merge(&mut self, source: TaskId, target: TaskId) -> Result<(), DomainError> {
        if source == target {
            return Err(DomainError::SameEntity);
        }
        let src = self.task(source)?;
        let dst = self.task(target)?;
        if src.merged_into.is_some() {
            return Err(DomainError::invalid("source", "already merged"));
        }
        // Steps go after the target's own steps, keeping their relative order.
        let mut last = self
            .state
            .siblings(Scope::Steps(target))
            .last()
            .map(|(k, _)| k.clone());
        for (_, sid) in self.state.siblings(Scope::Steps(source)) {
            let EntityId::Step(sid) = sid else { continue };
            let Some(s) = self.state.steps.get(&sid) else {
                continue;
            };
            let key = crate::ordering::PosKey::between(last.as_ref(), None)?;
            last = Some(key.clone());
            self.update(
                sid.into(),
                vec![
                    FieldChange::ParentTask(Change::new(source, target)),
                    FieldChange::Position(Change::new(s.position.clone(), key)),
                ],
            );
        }
        let moved = |this: &mut Self, ids: Vec<EntityId>| {
            for id in ids {
                this.update(
                    id,
                    vec![FieldChange::ParentTask(Change::new(source, target))],
                );
            }
        };
        let live = |deleted: &Option<Tombstone>| deleted.is_none();
        let ids = self
            .state
            .links
            .values()
            .filter(|l| l.task == source && live(&l.deleted))
            .map(|l| l.id.into())
            .collect();
        moved(self, ids);
        let ids = self
            .state
            .attachments
            .values()
            .filter(|l| l.task == source && live(&l.deleted))
            .map(|l| l.id.into())
            .collect();
        moved(self, ids);
        let ids = self
            .state
            .reminders
            .values()
            .filter(|l| l.task == source && live(&l.deleted))
            .map(|l| l.id.into())
            .collect();
        moved(self, ids);

        let mut ch: Vec<FieldChange> = src
            .tags
            .difference(&dst.tags)
            .map(|t| FieldChange::TagAdded { tag: *t })
            .collect();
        if !src.notes.trim().is_empty() {
            let merged = if dst.notes.is_empty() {
                src.notes.clone()
            } else {
                format!("{}\n\n{}", dst.notes, src.notes)
            };
            notes(&merged)?;
            ch.push(FieldChange::Notes(Change::new(dst.notes.clone(), merged)));
        }
        self.update_task(dst, ch);
        self.update_task(
            src,
            vec![FieldChange::MergedInto(Change::new(None, Some(target)))],
        );
        self.push(EventPayload::Deleted {
            target: source.into(),
            tombstone: Tombstone {
                at: self.now,
                by: self.actor.clone(),
            },
        });
        Ok(())
    }

    fn delete(&mut self, id: EntityId) -> Result<(), DomainError> {
        if !self.state.exists(id) {
            return Err(DomainError::NotFound(id));
        }
        if self.state.tombstone(id).is_some() {
            return Err(DomainError::AlreadyDeleted(id));
        }
        self.push(EventPayload::Deleted {
            target: id,
            tombstone: Tombstone {
                at: self.now,
                by: self.actor.clone(),
            },
        });
        Ok(())
    }

    fn restore(&mut self, id: EntityId) -> Result<(), DomainError> {
        let Some(e) = self.state.entity(id) else {
            return Err(DomainError::NotFound(id));
        };
        let Some(tombstone) = e.deleted().cloned() else {
            return Err(DomainError::NotDeleted(id));
        };
        if let Some(p) = e.parent()
            && !self.state.is_live(p)
        {
            return Err(DomainError::Deleted(p));
        }
        if let Entity::Tag(t) = &e {
            unique_tag_name(self.state, &t.name, Some(t.id))?;
        }
        self.push(EventPayload::Restored {
            target: id,
            tombstone,
        });
        // A live sibling may have taken the old key meanwhile; give the restored item a fresh one so keys
        // stay unique.
        if let (Some(scope), Some(key)) = (State::scope_of_entity(&e), e.position())
            && let Some(fresh) = self.state.fresh_key_if_taken(scope, id, key)?
        {
            let ch = vec![FieldChange::Position(Change::new(key.clone(), fresh))];
            match id {
                EntityId::Task(t) => {
                    let task = self.state.tasks.get(&t).ok_or(DomainError::NotFound(id))?;
                    self.update_task(task, ch);
                }
                _ => self.update(id, ch),
            }
        }
        Ok(())
    }

    fn reorder(&mut self, id: EntityId, pl: &Placement) -> Result<(), DomainError> {
        self.require_live(id)?;
        let scope = self
            .state
            .scope_of(id)
            .ok_or_else(|| DomainError::invalid("id", "this kind of entity has no order"))?;
        let pos = self.state.resolve_placement(scope, Some(id), pl)?;
        let (old, task) = match id {
            EntityId::Collection(i) => (
                self.state.collections.get(&i).map(|e| e.position.clone()),
                None,
            ),
            EntityId::List(i) => (self.state.lists.get(&i).map(|e| e.position.clone()), None),
            EntityId::Task(i) => (
                self.state.tasks.get(&i).map(|e| e.position.clone()),
                self.state.tasks.get(&i),
            ),
            EntityId::Step(i) => (self.state.steps.get(&i).map(|e| e.position.clone()), None),
            _ => (None, None),
        };
        let old = old.ok_or(DomainError::NotFound(id))?;
        if old == pos {
            return Ok(());
        }
        let ch = vec![FieldChange::Position(Change::new(old, pos))];
        match task {
            Some(t) => self.update_task(t, ch),
            None => self.update(id, ch),
        }
        Ok(())
    }

    /// Stores an imported entity with its own times (see [`Command::ImportEntity`]).
    fn import(&mut self, entity: &Entity) -> Result<(), DomainError> {
        let id = entity.id();
        self.require_new(id)?;
        if entity.deleted().is_some() {
            return Err(DomainError::invalid("deleted", "must not be set on import"));
        }
        if let Some(parent) = entity.parent() {
            self.require_live(parent)?;
        }
        let by = self.actor.clone();
        let mut e = entity.clone();
        match &mut e {
            Entity::Collection(Collection { name, created, .. })
            | Entity::List(List { name, created, .. }) => {
                *name = text("name", name, MAX_NAME)?;
                created.by = by;
            }
            Entity::Tag(t) => {
                t.name = text("name", &t.name, MAX_NAME)?;
                unique_tag_name(self.state, &t.name, None)?;
                t.created.by = by;
            }
            Entity::Task(t) => {
                if t.merged_into.is_some() {
                    return Err(DomainError::invalid(
                        "merged_into",
                        "must not be set on import",
                    ));
                }
                for tag in &t.tags {
                    self.require_live((*tag).into())?;
                }
                t.title = text("title", &t.title, MAX_TITLE)?;
                t.created.by = by.clone();
                t.updated.by = by;
            }
            Entity::Step(s) => {
                s.title = text("title", &s.title, MAX_TITLE)?;
                s.created.by = by;
            }
            Entity::Link(Link { created, .. })
            | Entity::Attachment(AttachmentRef { created, .. })
            | Entity::Reminder(Reminder { created, .. }) => created.by = by,
        }
        validate_entity(&e)?;
        if let (Some(scope), Some(key)) = (State::scope_of_entity(&e), e.position().cloned())
            && let Some(fresh) = self.state.fresh_key_if_taken(scope, id, &key)?
        {
            match &mut e {
                Entity::Collection(x) => x.position = fresh,
                Entity::List(x) => x.position = fresh,
                Entity::Task(x) => x.position = fresh,
                Entity::Step(x) => x.position = fresh,
                _ => {}
            }
        }
        self.push(EventPayload::Created { entity: e });
        Ok(())
    }

    fn run(&mut self, cmd: &Command) -> Result<(), DomainError> {
        use Command as C;
        let stamp = self.stamp();
        match cmd {
            C::CreateCollection {
                id,
                name,
                color: col,
                placement,
            } => {
                self.require_new((*id).into())?;
                let name = text("name", name, MAX_NAME)?;
                color(col)?;
                let position = self
                    .state
                    .resolve_placement(Scope::Collections, None, placement)?;
                self.push(EventPayload::Created {
                    entity: Entity::Collection(Collection {
                        id: *id,
                        name,
                        color: col.clone(),
                        task_sort: None,
                        position,
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::UpdateCollection {
                id,
                name,
                color: col,
            } => {
                self.require_live((*id).into())?;
                let c = self
                    .state
                    .collections
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let ch = self.name_changes(&c.name, &c.color, name, col, MAX_NAME)?;
                self.update((*id).into(), ch);
            }
            C::CreateList {
                id,
                collection,
                name,
                color: col,
                placement,
            } => {
                self.require_new((*id).into())?;
                if let Some(c) = collection {
                    self.require_live((*c).into())?;
                }
                let name = text("name", name, MAX_NAME)?;
                color(col)?;
                let position =
                    self.state
                        .resolve_placement(Scope::Lists(*collection), None, placement)?;
                self.push(EventPayload::Created {
                    entity: Entity::List(List {
                        id: *id,
                        collection: *collection,
                        name,
                        color: col.clone(),
                        features: Default::default(),
                        task_sort: None,
                        position,
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::UpdateList {
                id,
                name,
                color: col,
                features,
            } => {
                self.require_live((*id).into())?;
                let l = self
                    .state
                    .lists
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let mut ch = self.name_changes(&l.name, &l.color, name, col, MAX_NAME)?;
                if let Some(patch) = features {
                    let mut next = patch.merge(l.features);
                    if !next.allows(next.default_view) {
                        if l.features.default_view == next.default_view {
                            // Switching off the current default view falls back in the same event.
                            next.default_view = ListView::List;
                        } else {
                            return Err(DomainError::invalid(
                                "default_view",
                                "requires its feature to be enabled",
                            ));
                        }
                    }
                    if next != l.features {
                        ch.push(FieldChange::ListFeatures(Change::new(l.features, next)));
                    }
                }
                self.update((*id).into(), ch);
            }
            C::SetTaskSort { id, sort } => {
                self.require_live(*id)?;
                let (old, default) = match id {
                    EntityId::Collection(id) => {
                        (self.state.collections[id].task_sort, TaskSort::Newest)
                    }
                    EntityId::List(id) => (self.state.lists[id].task_sort, TaskSort::Manual),
                    _ => return Err(DomainError::invalid("id", "expected a collection or list")),
                };
                let new = (*sort != default).then_some(*sort);
                if old != new {
                    self.update(*id, vec![FieldChange::TaskSort(Change::new(old, new))]);
                }
            }
            C::MoveList {
                id,
                collection,
                placement,
            } => {
                self.require_live((*id).into())?;
                if let Some(c) = collection {
                    self.require_live((*c).into())?;
                }
                let l = self
                    .state
                    .lists
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let pos = self.state.resolve_placement(
                    Scope::Lists(*collection),
                    Some((*id).into()),
                    placement,
                )?;
                let mut ch = Vec::new();
                let other_collection = l.collection != *collection;
                if other_collection {
                    ch.push(FieldChange::ListCollection(Change::new(
                        l.collection,
                        *collection,
                    )));
                }
                // As with a task moved to another list: a move to another collection (or out of one)
                // always records the position, even a key that stays the same, so a later reorder
                // there makes undoing the move a conflict instead of taking the list back with the key
                // it got there, in the wrong place among its old siblings. Moves recorded before this
                // (the collection only) still apply and undo as they did.
                if other_collection || l.position != pos {
                    ch.push(FieldChange::Position(Change::new(l.position.clone(), pos)));
                }
                self.update((*id).into(), ch);
            }
            C::BulkTasks { .. } => unreachable!("bulk commands are decided separately"),
            C::CreateTask(n) => self.create_task(n)?,
            C::UpdateTask { id, patch } => self.update_task_fields(*id, patch)?,
            C::MoveTask {
                id,
                list,
                from_list,
                placement,
            } => self.move_task(*id, *list, *from_list, placement)?,
            C::MoveTaskStatus {
                id,
                list,
                status,
                placement,
            } => self.move_task_status(*id, *list, *status, *placement)?,
            C::CompleteTask { id } => self.set_completed(*id, true)?,
            C::UncompleteTask { id } => self.set_completed(*id, false)?,
            C::SetMyDay { id, date } => {
                let t = self.task(*id)?;
                if t.my_day != *date {
                    self.update_task(t, vec![FieldChange::MyDay(Change::new(t.my_day, *date))]);
                }
            }
            C::AddTag { task, tag } => {
                let t = self.task(*task)?;
                self.require_live((*tag).into())?;
                if !t.tags.contains(tag) {
                    self.update_task(t, vec![FieldChange::TagAdded { tag: *tag }]);
                }
            }
            C::RemoveTag { task, tag } => {
                let t = self.task(*task)?;
                if t.tags.contains(tag) {
                    self.update_task(t, vec![FieldChange::TagRemoved { tag: *tag }]);
                }
            }
            C::MergeTasks { source, target } => self.merge(*source, *target)?,
            C::AddNote { task, text: add } => {
                let t = self.task(*task)?;
                let add = text("text", add, MAX_NOTES)?;
                let new = if t.notes.is_empty() {
                    add
                } else {
                    format!("{}\n\n{add}", t.notes)
                };
                notes(&new)?;
                self.update_task(
                    t,
                    vec![FieldChange::NoteAdded(Change::new(t.notes.clone(), new))],
                );
            }
            C::CreateStep {
                id,
                task,
                title,
                placement,
            } => {
                self.require_new((*id).into())?;
                self.task(*task)?;
                let title = text("title", title, MAX_TITLE)?;
                let position =
                    self.state
                        .resolve_placement(Scope::Steps(*task), None, placement)?;
                self.push(EventPayload::Created {
                    entity: Entity::Step(Step {
                        id: *id,
                        task: *task,
                        title,
                        completed_at: None,
                        position,
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::UpdateStep { id, title } => {
                self.require_live((*id).into())?;
                let s = self
                    .state
                    .steps
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let title = text("title", title, MAX_TITLE)?;
                if title != s.title {
                    self.update(
                        (*id).into(),
                        vec![FieldChange::Title(Change::new(s.title.clone(), title))],
                    );
                }
            }
            C::CompleteStep { id } | C::UncompleteStep { id } => {
                self.require_live((*id).into())?;
                let s = self
                    .state
                    .steps
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let done = matches!(cmd, C::CompleteStep { .. });
                if s.completed_at.is_some() != done {
                    let new = done.then_some(self.now);
                    self.update(
                        (*id).into(),
                        vec![FieldChange::CompletedAt(Change::new(s.completed_at, new))],
                    );
                }
            }
            C::CreateTag {
                id,
                name,
                color: col,
            } => {
                self.require_new((*id).into())?;
                let name = text("name", name, MAX_NAME)?;
                color(col)?;
                unique_tag_name(self.state, &name, None)?;
                self.push(EventPayload::Created {
                    entity: Entity::Tag(Tag {
                        id: *id,
                        name,
                        color: col.clone(),
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::UpdateTag {
                id,
                name,
                color: col,
            } => {
                self.require_live((*id).into())?;
                let g = self
                    .state
                    .tags
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                let ch = self.name_changes(&g.name, &g.color, name, col, MAX_NAME)?;
                if let Some(FieldChange::Name(c)) = ch.first() {
                    unique_tag_name(self.state, &c.new, Some(*id))?;
                }
                self.update((*id).into(), ch);
            }
            C::AddLink { id, task, kind } => {
                self.require_new((*id).into())?;
                self.task(*task)?;
                validate_link(kind)?;
                self.push(EventPayload::Created {
                    entity: Entity::Link(Link {
                        id: *id,
                        task: *task,
                        kind: kind.clone(),
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::AddAttachment {
                id,
                task,
                file_name,
                media_type,
                size_bytes,
                storage_key,
            } => {
                self.require_new((*id).into())?;
                self.task(*task)?;
                short("file_name", file_name)?;
                if file_name.contains(['/', '\\']) {
                    return Err(DomainError::invalid("file_name", "must not contain a path"));
                }
                short("media_type", media_type)?;
                short("storage_key", storage_key)?;
                self.push(EventPayload::Created {
                    entity: Entity::Attachment(AttachmentRef {
                        id: *id,
                        task: *task,
                        file_name: file_name.clone(),
                        media_type: media_type.clone(),
                        size_bytes: *size_bytes,
                        storage_key: storage_key.clone(),
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::AddReminder { id, task, at } => {
                self.require_new((*id).into())?;
                self.task(*task)?;
                self.push(EventPayload::Created {
                    entity: Entity::Reminder(Reminder {
                        id: *id,
                        task: *task,
                        at: *at,
                        created: stamp,
                        deleted: None,
                    }),
                });
            }
            C::UpdateReminder { id, at } => {
                self.require_live((*id).into())?;
                let r = self
                    .state
                    .reminders
                    .get(id)
                    .ok_or(DomainError::NotFound((*id).into()))?;
                if r.at != *at {
                    self.update(
                        (*id).into(),
                        vec![FieldChange::ReminderAt(Change::new(r.at, *at))],
                    );
                }
            }
            C::ImportEntity { entity } => self.import(entity)?,
            C::Reorder { id, placement } => self.reorder(*id, placement)?,
            C::Delete { id } => self.delete(*id)?,
            C::Restore { id } => self.restore(*id)?,
        }
        Ok(())
    }
}

/// Validates `command` for `actor` against `state` and returns the events it causes.
///
/// Returns an empty list when the command would change nothing (for example completing a task that is
/// already complete). Does not modify `state`; apply the events with [`apply`](crate::apply::apply).
pub fn decide(
    state: &State,
    actor: &Actor,
    command: &Command,
    now: Timestamp,
) -> Result<Vec<Event>, DomainError> {
    decide_inner(state, actor, command, now, None)
}

/// Like [`decide`], with an additional host-supplied event ID namespace. Use distinct seeds for
/// commands within an atomic batch, so repeating the same command after an intervening change at
/// the same timestamp cannot reuse event IDs. Retrying with the same seed is deterministic.
pub fn decide_seeded(
    state: &State,
    actor: &Actor,
    command: &Command,
    now: Timestamp,
    seed: &str,
) -> Result<Vec<Event>, DomainError> {
    decide_inner(state, actor, command, now, Some(seed))
}

fn decide_inner(
    state: &State,
    actor: &Actor,
    command: &Command,
    now: Timestamp,
    namespace: Option<&str>,
) -> Result<Vec<Event>, DomainError> {
    check_limits(command)?;
    authorize(state, actor, command)?;
    let seed = match namespace {
        Some(namespace) => format!("batch|{namespace:?}|{actor:?}|{command:?}"),
        None => format!("{actor:?}|{command:?}"),
    };
    let mut events = if let Command::BulkTasks { tasks, action } = command {
        if let BulkTaskAction::Reorder { list, placement } = action {
            // Never relocate a partial group if one of its tasks moved or completed meanwhile.
            if tasks.iter().any(|id| {
                !state.is_live((*id).into())
                    || !state
                        .tasks
                        .get(id)
                        .is_some_and(|t| t.list == *list && !t.is_done())
            }) {
                return Err(DomainError::invalid(
                    "tasks",
                    "reorder requires open tasks in one list",
                ));
            }
            if matches!(placement, Placement::Before(EntityId::Task(id)) | Placement::After(EntityId::Task(id)) if tasks.contains(id))
            {
                return Err(DomainError::invalid(
                    "placement",
                    "cannot reorder relative to a selected task",
                ));
            }
        }
        let seed = EventId::derive(now, seed.as_bytes());
        let mut next = state.clone();
        let mut events = Vec::new();
        // Insertion at the front or immediately after a fixed sibling reverses successive
        // inserts. Walk the group backwards in those cases to preserve its input order.
        let mut tasks = tasks.iter();
        while let Some(&id) = if matches!(
            action,
            BulkTaskAction::Move { .. }
                | BulkTaskAction::Reorder {
                    placement: Placement::First | Placement::After(_),
                    ..
                }
        ) {
            tasks.next_back()
        } else {
            tasks.next()
        } {
            let mut d = Decider {
                state: &next,
                actor,
                now,
                seed: format!("{seed}|{id}"),
                events: Vec::new(),
            };
            if let Some(change) = action.change(d.task(id)?) {
                d.run(&change)?;
                for event in d.events {
                    apply(&mut next, &event)
                        .map_err(|_| DomainError::invalid("tasks", "cannot apply bulk change"))?;
                    events.push(event);
                }
            }
        }
        events
    } else {
        let mut d = Decider {
            state,
            actor,
            now,
            seed,
            events: Vec::new(),
        };
        d.run(command)?;
        d.events
    };
    if let Some(first) = events.first().map(|e| e.id) {
        for e in &mut events {
            e.group = first;
        }
    }
    Ok(events)
}
