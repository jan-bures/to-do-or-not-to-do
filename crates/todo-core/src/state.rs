//! The in-memory state that `decide`, `apply` and `undo` operate on.
//!
//! [`State`] is a set of plain maps (easy to project into SQLite) plus a [`Journal`] with the bookkeeping
//! undo needs: a fingerprint of every applied event, which are undone, and the provenance of each field's
//! current value.

use crate::access::Grants;
use crate::actor::{Actor, AgentId};
use crate::command::Placement;
use crate::error::{ApplyError, DomainError};
use crate::event::FieldKey;
use crate::ids::{
    AttachmentId, CollectionId, EntityId, EventId, LinkId, ListId, ReminderId, StepId, TagId,
    TaskId,
};
use crate::model::{
    AttachmentRef, Collection, Entity, Link, List, Reminder, Step, Tag, Task, Tombstone,
};
use crate::ordering::PosKey;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(
        map: &BTreeMap<K, V>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, D>(d: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, V)>::deserialize(d)?.into_iter().collect())
    }
}

/// The provenance of a field's current value: the ordinary event that wrote it, or `None` for the
/// value the entity was created with.
pub(crate) type Token = Option<EventId>;

/// One field written by an applied event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FieldWrite {
    /// The entity and field.
    pub(crate) key: (EntityId, FieldKey),
    /// Provenance of the value the event replaced.
    pub(crate) before: Token,
    /// Provenance of the value the event wrote.
    pub(crate) after: Token,
    /// For tag membership: whether the tag was set before the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tag_was_set: Option<bool>,
}

/// What the journal remembers about one applied event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Applied {
    /// Position in apply order.
    pub(crate) order: u64,
    /// Who caused it.
    pub(crate) actor: Actor,
    /// Fingerprint of everything but `seq` (see [`Event::fingerprint`](crate::event::Event::fingerprint)).
    pub(crate) fingerprint: [u8; 32],
    /// The fields it wrote.
    pub(crate) writes: Vec<FieldWrite>,
    /// Provenance of a task's last-modified stamp before and after the event, if it touched one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) touch: Option<(Token, Token)>,
    /// For a delete or restore: the entity's tombstone before the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tombstone_before: Option<Tombstone>,
}

impl Applied {
    pub(crate) fn write(&self, key: &(EntityId, FieldKey)) -> Option<&FieldWrite> {
        self.writes.iter().find(|w| &w.key == key)
    }
}

/// Undo bookkeeping. Maintained by [`apply`](crate::apply::apply); treat as opaque.
///
/// For every field it tracks the *provenance* of the current value: the ordinary event whose write is in
/// effect. Compensating events are recorded as writes too: an undo writes back the value the undone
/// event replaced, so the field's provenance returns to what it was before that event. An undo is
/// conflict-free only while the provenance is still what the undone event left behind.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// Every applied event, ordinary or compensating.
    #[serde(with = "pairs")]
    pub(crate) applied: BTreeMap<EventId, Applied>,
    /// Number of events applied so far.
    pub(crate) next_order: u64,
    /// Provenance of each field's current value (absent = the value from creation).
    #[serde(with = "pairs")]
    pub(crate) tokens: BTreeMap<(EntityId, FieldKey), EventId>,
    /// Provenance of each task's last-modified stamp (absent = the stamp from creation).
    #[serde(with = "pairs")]
    pub(crate) touch_tokens: BTreeMap<EntityId, EventId>,
    /// For each field, every event that wrote it (including compensations), oldest first.
    #[serde(with = "pairs")]
    pub(crate) field_log: BTreeMap<(EntityId, FieldKey), Vec<EventId>>,
    /// Ordinary events whose effect is currently undone.
    pub(crate) undone: BTreeSet<EventId>,
    /// Compensating event -> the event it undoes.
    #[serde(with = "pairs")]
    pub(crate) origin: BTreeMap<EventId, EventId>,
    /// Ordinary event -> the latest compensating event in its undo/redo chain.
    #[serde(with = "pairs")]
    pub(crate) heads: BTreeMap<EventId, EventId>,
    /// Every entity ID ever created -> the ordinary event that created it. Kept after the creation is
    /// undone: an ID is never given to a different entity, so every event naming an ID means one entity.
    #[serde(default, with = "pairs")]
    pub(crate) created: BTreeMap<EntityId, EventId>,
}

impl Journal {
    /// The ordinary event at the root of an undo/redo chain.
    #[must_use]
    pub fn root_of(&self, mut event: EventId) -> EventId {
        while let Some(&t) = self.origin.get(&event) {
            event = t;
        }
        event
    }

    /// True if the event was applied.
    #[must_use]
    pub fn has_seen(&self, event: EventId) -> bool {
        self.applied.contains_key(&event)
    }

    /// True if the ordinary event's effect is currently undone.
    #[must_use]
    pub fn is_undone(&self, root: EventId) -> bool {
        self.undone.contains(&root)
    }

    /// The latest event of the chain rooted at `root` (the root itself if never undone).
    #[must_use]
    pub fn head_of(&self, root: EventId) -> EventId {
        self.heads.get(&root).copied().unwrap_or(root)
    }

    /// True if the event is the newest link of its undo/redo chain (it has not been undone).
    #[must_use]
    pub fn is_head(&self, event: EventId) -> bool {
        self.has_seen(event) && self.head_of(self.root_of(event)) == event
    }

    /// The events that wrote `field` after `event` and have not been undone, oldest first.
    ///
    /// Compensating events count as writes.
    #[must_use]
    pub fn active_after(&self, key: &(EntityId, FieldKey), event: EventId) -> Vec<EventId> {
        let Some(log) = self.field_log.get(key) else {
            return Vec::new();
        };
        let pos = log.iter().rposition(|e| *e == event).map_or(0, |p| p + 1);
        log[pos..]
            .iter()
            .copied()
            .filter(|e| self.is_head(*e))
            .collect()
    }

    pub(crate) fn token(&self, key: &(EntityId, FieldKey)) -> Token {
        self.tokens.get(key).copied()
    }

    pub(crate) fn order_of(&self, event: EventId) -> Option<u64> {
        self.applied.get(&event).map(|a| a.order)
    }

    /// True if an entity with this ID was ever created, even if the creation was undone since. Such an ID
    /// cannot be used for a new entity.
    #[must_use]
    pub fn id_used(&self, id: EntityId) -> bool {
        self.created.contains_key(&id)
    }

    /// The ordinary event that created the entity with this ID, if any.
    pub(crate) fn creation_of(&self, id: EntityId) -> Option<EventId> {
        self.created.get(&id).copied()
    }

    /// Provenance the field had once the event at position `order` in apply order had been applied.
    fn token_at(&self, key: &(EntityId, FieldKey), log: &[EventId], order: u64) -> Token {
        log.iter()
            .rev()
            .find(|e| self.order_of(**e).is_some_and(|o| o <= order))
            .and_then(|e| self.applied.get(e))
            .and_then(|a| a.write(key))
            .and_then(|w| w.after)
    }

    /// Fields of `entity` whose value changed after the event at position `since` in apply order, by
    /// any event (ordinary or compensating), each with the still-active events that wrote it since. A
    /// field counts if its provenance now differs from its provenance right after that event, so a write
    /// that was undone again does not count. Its existence counts only if `include_existence` is set.
    pub(crate) fn changed_since(
        &self,
        entity: EntityId,
        since: u64,
        include_existence: bool,
    ) -> Vec<((EntityId, FieldKey), Vec<EventId>)> {
        self.field_log
            .range((entity, FieldKey::Existence)..)
            .take_while(|((e, _), _)| *e == entity)
            .filter(|((_, f), _)| include_existence || *f != FieldKey::Existence)
            .filter(|(key, log)| self.token(key) != self.token_at(key, log, since))
            .map(|(key, log)| {
                let mut later: Vec<EventId> = log
                    .iter()
                    .copied()
                    .filter(|e| self.order_of(*e).is_some_and(|o| o > since) && self.is_head(*e))
                    .collect();
                if later.is_empty() {
                    later.extend(self.token(key));
                }
                (key.clone(), later)
            })
            .collect()
    }
}

/// Groups of siblings that carry position keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// All collections.
    Collections,
    /// Lists of one collection (`None` = top level).
    Lists(Option<CollectionId>),
    /// Tasks of a list.
    Tasks(ListId),
    /// Steps of a task.
    Steps(TaskId),
}

/// All entities plus bookkeeping.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Collections.
    pub collections: BTreeMap<CollectionId, Collection>,
    /// Lists.
    pub lists: BTreeMap<ListId, List>,
    /// Tasks.
    pub tasks: BTreeMap<TaskId, Task>,
    /// Steps.
    pub steps: BTreeMap<StepId, Step>,
    /// Tags.
    pub tags: BTreeMap<TagId, Tag>,
    /// Links.
    pub links: BTreeMap<LinkId, Link>,
    /// Attachment references.
    pub attachments: BTreeMap<AttachmentId, AttachmentRef>,
    /// Reminders.
    pub reminders: BTreeMap<ReminderId, Reminder>,
    /// Agent access grants. Managed by the server (not event-sourced).
    pub grants: BTreeMap<AgentId, Grants>,
    /// Undo bookkeeping.
    pub journal: Journal,
}

impl State {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True if the entity maps are equal, ignoring grants and the journal.
    #[must_use]
    pub fn data_eq(&self, other: &Self) -> bool {
        self.collections == other.collections
            && self.lists == other.lists
            && self.tasks == other.tasks
            && self.steps == other.steps
            && self.tags == other.tags
            && self.links == other.links
            && self.attachments == other.attachments
            && self.reminders == other.reminders
    }

    /// Actor who set the task's current completion time, if the replica knows it.
    /// A later edit does not change it, and undo/redo restores the original field provenance.
    /// Imports can have a completion time at creation without recording who completed the task.
    #[must_use]
    pub fn task_completion_actor(&self, id: TaskId) -> Option<&Actor> {
        self.tasks.get(&id)?.completed_at?;
        let event = self.journal.token(&(id.into(), FieldKey::CompletedAt))?;
        self.journal.applied.get(&event).map(|entry| &entry.actor)
    }

    /// A copy of the entity, if it exists (deleted or not).
    #[must_use]
    pub fn entity(&self, id: EntityId) -> Option<Entity> {
        match id {
            EntityId::Collection(i) => self.collections.get(&i).cloned().map(Entity::Collection),
            EntityId::List(i) => self.lists.get(&i).cloned().map(Entity::List),
            EntityId::Task(i) => self.tasks.get(&i).cloned().map(Entity::Task),
            EntityId::Step(i) => self.steps.get(&i).cloned().map(Entity::Step),
            EntityId::Tag(i) => self.tags.get(&i).cloned().map(Entity::Tag),
            EntityId::Link(i) => self.links.get(&i).cloned().map(Entity::Link),
            EntityId::Attachment(i) => self.attachments.get(&i).cloned().map(Entity::Attachment),
            EntityId::Reminder(i) => self.reminders.get(&i).cloned().map(Entity::Reminder),
        }
    }

    /// True if the entity exists, whether or not it is deleted.
    #[must_use]
    pub fn exists(&self, id: EntityId) -> bool {
        match id {
            EntityId::Collection(i) => self.collections.contains_key(&i),
            EntityId::List(i) => self.lists.contains_key(&i),
            EntityId::Task(i) => self.tasks.contains_key(&i),
            EntityId::Step(i) => self.steps.contains_key(&i),
            EntityId::Tag(i) => self.tags.contains_key(&i),
            EntityId::Link(i) => self.links.contains_key(&i),
            EntityId::Attachment(i) => self.attachments.contains_key(&i),
            EntityId::Reminder(i) => self.reminders.contains_key(&i),
        }
    }

    /// The entity's own tombstone (`None` if it exists and is not deleted, or does not exist).
    #[must_use]
    pub fn tombstone(&self, id: EntityId) -> Option<Tombstone> {
        self.entity(id).and_then(|e| e.deleted().cloned())
    }

    /// True if the entity exists, is not deleted and none of its ancestors is deleted.
    #[must_use]
    pub fn is_live(&self, id: EntityId) -> bool {
        match self.entity(id) {
            Some(e) => e.deleted().is_none() && e.parent().is_none_or(|p| self.is_live(p)),
            None => false,
        }
    }

    /// Sets or clears an entity's tombstone.
    pub fn set_tombstone(&mut self, id: EntityId, t: Option<Tombstone>) -> Result<(), ApplyError> {
        let nf = ApplyError::NotFound(id);
        match id {
            EntityId::Collection(i) => self.collections.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::List(i) => self.lists.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Task(i) => self.tasks.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Step(i) => self.steps.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Tag(i) => self.tags.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Link(i) => self.links.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Attachment(i) => self.attachments.get_mut(&i).ok_or(nf)?.deleted = t,
            EntityId::Reminder(i) => self.reminders.get_mut(&i).ok_or(nf)?.deleted = t,
        }
        Ok(())
    }

    /// Inserts an entity, failing if its ID is taken.
    pub fn insert(&mut self, entity: Entity) -> Result<(), ApplyError> {
        let id = entity.id();
        if self.exists(id) {
            return Err(ApplyError::AlreadyExists(id));
        }
        match entity {
            Entity::Collection(e) => self.collections.insert(e.id, e).map(drop),
            Entity::List(e) => self.lists.insert(e.id, e).map(drop),
            Entity::Task(e) => self.tasks.insert(e.id, e).map(drop),
            Entity::Step(e) => self.steps.insert(e.id, e).map(drop),
            Entity::Tag(e) => self.tags.insert(e.id, e).map(drop),
            Entity::Link(e) => self.links.insert(e.id, e).map(drop),
            Entity::Attachment(e) => self.attachments.insert(e.id, e).map(drop),
            Entity::Reminder(e) => self.reminders.insert(e.id, e).map(drop),
        };
        Ok(())
    }

    /// Overwrites an existing entity with a new value of the same ID.
    pub(crate) fn replace(&mut self, entity: Entity) -> Result<(), ApplyError> {
        let id = entity.id();
        if !self.exists(id) {
            return Err(ApplyError::NotFound(id));
        }
        match entity {
            Entity::Collection(e) => self.collections.insert(e.id, e).map(drop),
            Entity::List(e) => self.lists.insert(e.id, e).map(drop),
            Entity::Task(e) => self.tasks.insert(e.id, e).map(drop),
            Entity::Step(e) => self.steps.insert(e.id, e).map(drop),
            Entity::Tag(e) => self.tags.insert(e.id, e).map(drop),
            Entity::Link(e) => self.links.insert(e.id, e).map(drop),
            Entity::Attachment(e) => self.attachments.insert(e.id, e).map(drop),
            Entity::Reminder(e) => self.reminders.insert(e.id, e).map(drop),
        };
        Ok(())
    }

    /// Removes an entity from the maps (only used to withdraw a creation).
    pub fn remove(&mut self, id: EntityId) -> Result<(), ApplyError> {
        let found = match id {
            EntityId::Collection(i) => self.collections.remove(&i).is_some(),
            EntityId::List(i) => self.lists.remove(&i).is_some(),
            EntityId::Task(i) => self.tasks.remove(&i).is_some(),
            EntityId::Step(i) => self.steps.remove(&i).is_some(),
            EntityId::Tag(i) => self.tags.remove(&i).is_some(),
            EntityId::Link(i) => self.links.remove(&i).is_some(),
            EntityId::Attachment(i) => self.attachments.remove(&i).is_some(),
            EntityId::Reminder(i) => self.reminders.remove(&i).is_some(),
        };
        if found {
            Ok(())
        } else {
            Err(ApplyError::NotFound(id))
        }
    }

    /// Entities that reference `id` as their parent or tag (so it cannot simply be removed).
    #[must_use]
    pub fn dependents(&self, id: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        match id {
            EntityId::Collection(c) => {
                out.extend(
                    self.lists
                        .values()
                        .filter(|l| l.collection == Some(c))
                        .map(|l| EntityId::List(l.id)),
                );
            }
            EntityId::List(l) => {
                out.extend(
                    self.tasks
                        .values()
                        .filter(|t| t.list == l)
                        .map(|t| EntityId::Task(t.id)),
                );
            }
            EntityId::Task(t) => {
                out.extend(
                    self.steps
                        .values()
                        .filter(|s| s.task == t)
                        .map(|s| EntityId::Step(s.id)),
                );
                out.extend(
                    self.links
                        .values()
                        .filter(|s| s.task == t)
                        .map(|s| EntityId::Link(s.id)),
                );
                out.extend(
                    self.attachments
                        .values()
                        .filter(|s| s.task == t)
                        .map(|s| EntityId::Attachment(s.id)),
                );
                out.extend(
                    self.reminders
                        .values()
                        .filter(|s| s.task == t)
                        .map(|s| EntityId::Reminder(s.id)),
                );
                out.extend(
                    self.tasks
                        .values()
                        .filter(|s| s.merged_into == Some(t))
                        .map(|s| EntityId::Task(s.id)),
                );
            }
            EntityId::Tag(g) => {
                out.extend(
                    self.tasks
                        .values()
                        .filter(|t| t.tags.contains(&g))
                        .map(|t| EntityId::Task(t.id)),
                );
            }
            EntityId::Step(_)
            | EntityId::Link(_)
            | EntityId::Attachment(_)
            | EntityId::Reminder(_) => {}
        }
        out
    }

    /// Entities hidden when `id` is tombstoned: its children, their children, and so on.
    #[must_use]
    pub fn descendants(&self, id: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut todo = vec![id];
        while let Some(cur) = todo.pop() {
            let children: Vec<EntityId> = match cur {
                EntityId::Collection(c) => self
                    .lists
                    .values()
                    .filter(|l| l.collection == Some(c))
                    .map(|l| l.id.into())
                    .collect(),
                EntityId::List(l) => self
                    .tasks
                    .values()
                    .filter(|t| t.list == l)
                    .map(|t| t.id.into())
                    .collect(),
                EntityId::Task(t) => self
                    .dependents(cur)
                    .into_iter()
                    .filter(|d| self.entity(*d).and_then(|e| e.parent()) == Some(t.into()))
                    .collect(),
                _ => Vec::new(),
            };
            todo.extend(children.iter().copied());
            out.extend(children);
        }
        out
    }

    /// The sibling group an entity value would be ordered in.
    #[must_use]
    pub fn scope_of_entity(entity: &Entity) -> Option<Scope> {
        match entity {
            Entity::Collection(_) => Some(Scope::Collections),
            Entity::List(l) => Some(Scope::Lists(l.collection)),
            Entity::Task(t) => Some(Scope::Tasks(t.list)),
            Entity::Step(s) => Some(Scope::Steps(s.task)),
            _ => None,
        }
    }

    /// If a live sibling (other than `id`) already uses `key`, a fresh key just before it; else `None`.
    pub(crate) fn fresh_key_if_taken(
        &self,
        scope: Scope,
        id: EntityId,
        key: &PosKey,
    ) -> Result<Option<PosKey>, DomainError> {
        let sibs: Vec<_> = self
            .siblings(scope)
            .into_iter()
            .filter(|(_, x)| *x != id)
            .collect();
        if !sibs.iter().any(|(k, _)| k == key) {
            return Ok(None);
        }
        let lower = sibs.iter().rev().map(|(k, _)| k).find(|k| *k < key);
        Ok(Some(PosKey::between(lower, Some(key))?))
    }

    /// The sibling group an entity is ordered in.
    #[must_use]
    pub fn scope_of(&self, id: EntityId) -> Option<Scope> {
        match id {
            EntityId::Collection(_) => Some(Scope::Collections),
            EntityId::List(i) => self.lists.get(&i).map(|l| Scope::Lists(l.collection)),
            EntityId::Task(i) => self.tasks.get(&i).map(|t| Scope::Tasks(t.list)),
            EntityId::Step(i) => self.steps.get(&i).map(|s| Scope::Steps(s.task)),
            _ => None,
        }
    }

    /// Non-deleted siblings of a scope with their keys, in display order.
    #[must_use]
    pub fn siblings(&self, scope: Scope) -> Vec<(PosKey, EntityId)> {
        let mut v: Vec<(PosKey, EntityId)> = match scope {
            Scope::Collections => self
                .collections
                .values()
                .filter(|c| c.deleted.is_none())
                .map(|c| (c.position.clone(), c.id.into()))
                .collect(),
            Scope::Lists(col) => self
                .lists
                .values()
                .filter(|l| l.collection == col && l.deleted.is_none())
                .map(|l| (l.position.clone(), l.id.into()))
                .collect(),
            Scope::Tasks(list) => self
                .tasks
                .values()
                .filter(|t| t.list == list && t.deleted.is_none())
                .map(|t| (t.position.clone(), t.id.into()))
                .collect(),
            Scope::Steps(task) => self
                .steps
                .values()
                .filter(|s| s.task == task && s.deleted.is_none())
                .map(|s| (s.position.clone(), s.id.into()))
                .collect(),
        };
        v.sort();
        v
    }

    /// Computes the key for placing an item among the siblings of `scope`.
    ///
    /// `exclude` is the item being moved (so it does not count as its own neighbour). Siblings may share a
    /// key (for example after an undo restored an item whose key was reused); placing next to such a
    /// group puts the new key outside the whole group, so this never fails because of ties.
    pub fn resolve_placement(
        &self,
        scope: Scope,
        exclude: Option<EntityId>,
        placement: &Placement,
    ) -> Result<PosKey, DomainError> {
        let sibs: Vec<_> = self
            .siblings(scope)
            .into_iter()
            .filter(|(_, id)| Some(*id) != exclude)
            .collect();
        let find = |x: EntityId| {
            sibs.iter()
                .position(|(_, id)| *id == x)
                .ok_or(DomainError::BadPlacement(x))
        };
        let (lo, hi) = match placement {
            Placement::First => (None, sibs.first().map(|s| &s.0)),
            Placement::Last => (sibs.last().map(|s| &s.0), None),
            Placement::Before(x) => {
                let i = find(*x)?;
                let hi = &sibs[i].0;
                let lo = sibs[..i].iter().rev().map(|s| &s.0).find(|k| *k < hi);
                (lo, Some(hi))
            }
            Placement::After(x) => {
                let i = find(*x)?;
                let lo = &sibs[i].0;
                let hi = sibs[i + 1..].iter().map(|s| &s.0).find(|k| *k > lo);
                (Some(lo), hi)
            }
        };
        Ok(PosKey::between(lo, hi)?)
    }
}
