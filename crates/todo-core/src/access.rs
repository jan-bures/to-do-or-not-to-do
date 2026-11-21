//! Agent access levels and permission checks.
//!
//! Levels, from least to most: `None`, `Read`, `Capture` (create tasks, add notes, links and
//! attachments), `Plan` (change and organise tasks). A queue level is reserved for later and not
//! implemented. Agents get a level per list or collection through [`Grants`]; the most specific wins
//! (list, then collection, then default). Changing the structure (collections, lists, tags) is reserved
//! for the owner, except that an agent may create a tag ([`tag_creation`]). Non-agent actors have fixed rights: the owner, the importer and the system may do
//! everything, email ingest may capture.

use crate::actor::{Actor, AgentId};
use crate::command::Command;
use crate::error::{AccessDenied, UndoError};
use crate::event::{Event, EventPayload, FieldChange};
use crate::ids::{CollectionId, EntityId, ListId, TaskId};
use crate::model::Entity;
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much an agent may do. Ordered: `None < Read < Capture < Plan`.
///
/// A future `Queue` level (claim and work tasks) is reserved and intentionally absent.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    /// No access at all.
    #[default]
    None,
    /// May read.
    Read,
    /// May also create tasks and add notes, links and attachments.
    Capture,
    /// May also change, complete, move, reorder and delete tasks and steps.
    Plan,
}

/// An agent's access levels: a default plus overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    /// Level for lists and collections without an override.
    pub default: AccessLevel,
    /// Per-collection overrides.
    #[serde(with = "pairs")]
    pub collections: BTreeMap<CollectionId, AccessLevel>,
    /// Per-list overrides (beat the collection override).
    #[serde(with = "pairs")]
    pub lists: BTreeMap<ListId, AccessLevel>,
}

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

impl Grants {
    /// Grants with only a default level.
    #[must_use]
    pub fn with_default(default: AccessLevel) -> Self {
        Self {
            default,
            ..Self::default()
        }
    }

    /// Level on a list: the list override, else the collection override, else the default.
    #[must_use]
    pub fn level_for_list(&self, list: ListId, collection: Option<CollectionId>) -> AccessLevel {
        self.lists
            .get(&list)
            .or_else(|| collection.and_then(|c| self.collections.get(&c)))
            .copied()
            .unwrap_or(self.default)
    }

    /// Level on a collection: its override, else the default.
    #[must_use]
    pub fn level_for_collection(&self, collection: CollectionId) -> AccessLevel {
        self.collections
            .get(&collection)
            .copied()
            .unwrap_or(self.default)
    }

    /// The highest level granted anywhere.
    #[must_use]
    pub fn max_level(&self) -> AccessLevel {
        self.lists
            .values()
            .chain(self.collections.values())
            .copied()
            .fold(self.default, AccessLevel::max)
    }
}

/// What a command touches, for permission purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Not tied to a list or collection (tags, or an entity that could not be resolved).
    Global,
    /// A collection.
    Collection(CollectionId),
    /// A list (and the tasks in it).
    List(ListId),
}

/// What a command needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// At least `Read` on the target.
    Read,
    /// At least `Capture` on the target.
    Capture,
    /// At least `Plan` on the target.
    Plan,
    /// `Plan` on at least one target (used for creating tags, see [`tag_creation`]).
    PlanAnywhere,
    /// Owner-level: structural changes agents may never make.
    Structure,
    /// The owner in person, and no other actor: not agents, and not the `Importer` or `System`
    /// actors either. Used for [`Command::ImportEntity`], which writes historical stamps and is
    /// run by the owner's own import tool over the owner's token.
    OwnerOnly,
}

/// The level an actor has on a target. Agents get their grant; others get a fixed level.
#[must_use]
pub fn level_on(state: &State, actor: &Actor, target: Target) -> AccessLevel {
    match actor {
        Actor::Owner { .. } | Actor::Importer | Actor::System => AccessLevel::Plan,
        Actor::EmailIngest => AccessLevel::Capture,
        Actor::Agent { agent_id } => agent_level(state, agent_id, target),
    }
}

fn agent_level(state: &State, agent: &AgentId, target: Target) -> AccessLevel {
    let Some(g) = state.grants.get(agent) else {
        return AccessLevel::None;
    };
    match target {
        Target::Global => g.default,
        Target::Collection(c) => g.level_for_collection(c),
        Target::List(l) => {
            let col = state.lists.get(&l).and_then(|l| l.collection);
            g.level_for_list(l, col)
        }
    }
}

/// Checks one requirement. Pure: reads only `state`.
pub fn check(state: &State, actor: &Actor, need: Need, target: Target) -> Result<(), AccessDenied> {
    let full = matches!(actor, Actor::Owner { .. } | Actor::Importer | Actor::System);
    let have = match (need, actor) {
        (Need::PlanAnywhere, Actor::Agent { agent_id }) => state
            .grants
            .get(agent_id)
            .map_or(AccessLevel::None, Grants::max_level),
        _ => level_on(state, actor, target),
    };
    let (needed, ok, owner_only) = match need {
        Need::Read => (AccessLevel::Read, have >= AccessLevel::Read, false),
        Need::Capture => (AccessLevel::Capture, have >= AccessLevel::Capture, false),
        Need::Plan | Need::PlanAnywhere => (AccessLevel::Plan, have >= AccessLevel::Plan, false),
        Need::Structure => (AccessLevel::Plan, full, true),
        Need::OwnerOnly => (AccessLevel::Plan, actor.is_owner(), true),
    };
    if ok {
        Ok(())
    } else {
        Err(AccessDenied {
            needed,
            have,
            owner_only,
        })
    }
}

fn task_target(state: &State, task: TaskId) -> Target {
    state
        .tasks
        .get(&task)
        .map_or(Target::Global, |t| Target::List(t.list))
}

/// The list or collection an entity falls under for permission purposes.
#[must_use]
pub fn entity_target(state: &State, id: EntityId) -> Target {
    match id {
        EntityId::Collection(c) => Target::Collection(c),
        EntityId::List(l) => Target::List(l),
        EntityId::Task(t) => task_target(state, t),
        EntityId::Step(s) => state
            .steps
            .get(&s)
            .map_or(Target::Global, |s| task_target(state, s.task)),
        EntityId::Link(s) => state
            .links
            .get(&s)
            .map_or(Target::Global, |s| task_target(state, s.task)),
        EntityId::Attachment(s) => state
            .attachments
            .get(&s)
            .map_or(Target::Global, |s| task_target(state, s.task)),
        EntityId::Reminder(s) => state
            .reminders
            .get(&s)
            .map_or(Target::Global, |s| task_target(state, s.task)),
        EntityId::Tag(_) => Target::Global,
    }
}

fn structural(id: EntityId) -> bool {
    matches!(
        id,
        EntityId::Collection(_) | EntityId::List(_) | EntityId::Tag(_)
    )
}

/// What creating a tag (or undoing a tag's withdrawal) needs: `Plan` on at least one list, and at least
/// `Read` at the global level (an agent's default level), in that order.
///
/// Tags are global, and an agent below `Read` there sees only the tags on tasks it can read. Without the
/// second requirement such an agent could create a tag it can't see, and the unique-name check would tell
/// it which names hidden tags use. Access is checked before that name check, so the agent gets
/// `forbidden` whatever name it asks for.
#[must_use]
pub fn tag_creation() -> Vec<(Need, Target)> {
    vec![
        (Need::PlanAnywhere, Target::Global),
        (Need::Read, Target::Global),
    ]
}

/// Everything a command needs, as `(need, target)` pairs.
#[must_use]
pub fn requirements(state: &State, cmd: &Command) -> Vec<(Need, Target)> {
    use Command as C;
    let on_task = |t: &TaskId| task_target(state, *t);
    match cmd {
        C::CreateCollection { id, .. } | C::UpdateCollection { id, .. } => {
            vec![(Need::Structure, Target::Collection(*id))]
        }
        C::CreateList { id, .. } | C::UpdateList { id, .. } | C::MoveList { id, .. } => {
            vec![(Need::Structure, Target::List(*id))]
        }
        C::CreateTask(n) => {
            let mut v = vec![(Need::Capture, Target::List(n.list))];
            if !n.tags.is_empty() || n.my_day.is_some() || n.recurrence.is_some() {
                v.push((Need::Plan, Target::List(n.list)));
            }
            v
        }
        C::UpdateTask { id, .. }
        | C::CompleteTask { id }
        | C::UncompleteTask { id }
        | C::SetMyDay { id, .. } => vec![(Need::Plan, on_task(id))],
        C::AddTag { task, .. } | C::RemoveTag { task, .. } => vec![(Need::Plan, on_task(task))],
        C::MoveTask { id, list, .. } => {
            vec![(Need::Plan, on_task(id)), (Need::Plan, Target::List(*list))]
        }
        C::MergeTasks { source, target } => {
            vec![(Need::Plan, on_task(source)), (Need::Plan, on_task(target))]
        }
        C::AddNote { task, .. } | C::AddLink { task, .. } | C::AddAttachment { task, .. } => {
            vec![(Need::Capture, on_task(task))]
        }
        C::CreateStep { task, .. } | C::AddReminder { task, .. } => {
            vec![(Need::Plan, on_task(task))]
        }
        C::UpdateStep { id, .. } | C::CompleteStep { id } | C::UncompleteStep { id } => {
            vec![(Need::Plan, entity_target(state, EntityId::Step(*id)))]
        }
        C::UpdateReminder { id, .. } => {
            vec![(Need::Plan, entity_target(state, EntityId::Reminder(*id)))]
        }
        C::CreateTag { .. } => tag_creation(),
        C::UpdateTag { .. } => vec![(Need::Structure, Target::Global)],
        C::ImportEntity { entity } => vec![(Need::OwnerOnly, value_target(state, entity))],
        C::Reorder { id, .. } | C::Delete { id } | C::Restore { id } => {
            let need = if structural(*id) {
                Need::Structure
            } else {
                Need::Plan
            };
            vec![(need, entity_target(state, *id))]
        }
    }
}

/// Decides whether `actor` may run `cmd`. Pure.
pub fn authorize(state: &State, actor: &Actor, cmd: &Command) -> Result<(), AccessDenied> {
    for (need, target) in requirements(state, cmd) {
        check(state, actor, need, target)?;
    }
    Ok(())
}

/// The list or collection an entity value falls under (for values not, or no longer, in the state).
fn value_target(state: &State, entity: &Entity) -> Target {
    match entity {
        Entity::Collection(c) => Target::Collection(c.id),
        Entity::List(l) => Target::List(l.id),
        Entity::Task(t) => Target::List(t.list),
        Entity::Step(x) => task_target(state, x.task),
        Entity::Link(x) => task_target(state, x.task),
        Entity::Attachment(x) => task_target(state, x.task),
        Entity::Reminder(x) => task_target(state, x.task),
        Entity::Tag(_) => Target::Global,
    }
}

/// What creating (or withdrawing) this entity needs, mirroring the create commands.
fn creation_requirements(state: &State, entity: &Entity) -> Vec<(Need, Target)> {
    let target = value_target(state, entity);
    match entity {
        Entity::Collection(_) | Entity::List(_) => vec![(Need::Structure, target)],
        Entity::Tag(_) => tag_creation(),
        Entity::Task(t) => {
            let mut v = vec![(Need::Capture, target)];
            if !t.tags.is_empty() || t.my_day.is_some() || t.recurrence.is_some() {
                v.push((Need::Plan, target));
            }
            v
        }
        Entity::Step(_) | Entity::Reminder(_) => vec![(Need::Plan, target)],
        Entity::Link(_) | Entity::Attachment(_) => vec![(Need::Capture, target)],
    }
}

/// Everything a compensating payload needs, as `(need, target)` pairs: the level the equivalent command
/// would need, on every list or collection the payload touches (for a move or a merge, both ends).
///
/// Changing notes needs plan access ([`FieldChange::Notes`] may replace or clear them). The one exception
/// is [`FieldChange::NoteAdded`], which only `AddNote` (capture access) produces: undoing or redoing that
/// note needs capture access, provided the notes are still exactly what the undone event left.
/// [`authorize_undo`] checks that proviso (it needs the undone event); this function assumes it.
#[must_use]
pub fn undo_requirements(state: &State, inverse: &EventPayload) -> Vec<(Need, Target)> {
    match inverse {
        EventPayload::Created { entity } | EventPayload::Withdrawn { entity } => {
            creation_requirements(state, entity)
        }
        EventPayload::Deleted { target, .. } | EventPayload::Restored { target, .. } => {
            let need = if structural(*target) {
                Need::Structure
            } else {
                Need::Plan
            };
            vec![(need, entity_target(state, *target))]
        }
        EventPayload::Updated { target, changes } => {
            let here = entity_target(state, *target);
            if structural(*target) {
                return vec![(Need::Structure, here)];
            }
            let mut v = vec![(Need::Capture, here)];
            for c in changes {
                match c {
                    FieldChange::Touch(_) => {}
                    // A note added with capture access can be taken back (or re-added) with it.
                    FieldChange::NoteAdded(_) => {}
                    FieldChange::TaskList(c) => {
                        v.push((Need::Plan, Target::List(c.old)));
                        v.push((Need::Plan, Target::List(c.new)));
                    }
                    FieldChange::ParentTask(c) => {
                        v.push((Need::Plan, task_target(state, c.old)));
                        v.push((Need::Plan, task_target(state, c.new)));
                    }
                    // Merging needs plan access on both tasks; so does taking it back.
                    FieldChange::MergedInto(c) => {
                        v.push((Need::Plan, here));
                        for t in [c.old, c.new].into_iter().flatten() {
                            v.push((Need::Plan, task_target(state, t)));
                        }
                    }
                    _ => v.push((Need::Plan, here)),
                }
            }
            v
        }
    }
}

/// True if every [`FieldChange::NoteAdded`] in `inverse` exactly takes back (or re-adds) the note
/// `undone` wrote, i.e. the notes still hold what `undone` left.
fn notes_untouched(undone: &EventPayload, inverse: &EventPayload) -> bool {
    let EventPayload::Updated { changes, .. } = inverse else {
        return true;
    };
    let wrote = match undone {
        EventPayload::Updated { changes, .. } => changes.iter().find_map(|c| match c {
            FieldChange::NoteAdded(c) => Some(&c.new),
            _ => None,
        }),
        _ => None,
    };
    changes.iter().all(|c| match c {
        FieldChange::NoteAdded(c) => wrote == Some(&c.old),
        _ => true,
    })
}

/// True if `event` is a rekey: the position update [`undo`](crate::undo::undo) adds to a compensation (in
/// its group) to keep keys unique.
pub(crate) fn is_rekey(state: &State, event: &Event) -> bool {
    event.undoes.is_none()
        && event.group != event.id
        && state.journal.origin.contains_key(&event.group)
        && matches!(&event.payload, EventPayload::Updated { changes, .. }
            if matches!(changes.as_slice(), [FieldChange::Position(_)]))
}

/// Checks that `actor` may apply `inverse`, the compensation for `undone`. The owner may do anything.
pub(crate) fn authorize_inverse(
    state: &State,
    actor: &Actor,
    undone: &Event,
    inverse: &EventPayload,
) -> Result<(), AccessDenied> {
    if actor.is_owner() {
        return Ok(());
    }
    let target = inverse.target();
    let mut needs = undo_requirements(state, inverse);
    // A rekey is part of a compensation the actor was allowed to make; taking it back needs no more.
    if is_rekey(state, undone) && !structural(target) {
        needs = vec![(Need::Capture, entity_target(state, target))];
    }
    // Overwriting later changes to the notes is no longer just taking back one's own note.
    if !notes_untouched(&undone.payload, inverse) {
        needs.push((Need::Plan, entity_target(state, target)));
    }
    for (need, target) in needs {
        check(state, actor, need, target)?;
    }
    Ok(())
}

/// Decides whether `actor` may undo `event`: the owner may undo anything; everyone else only their own
/// events, and only with the access the compensating change needs on every list or collection it
/// touches (see [`undo_requirements`]).
///
/// `event` must be the applied event ([`UndoError::EventMismatch`] otherwise).
pub fn authorize_undo(state: &State, actor: &Actor, event: &Event) -> Result<(), UndoError> {
    let applied = crate::undo::verified(state, event)?;
    if actor.is_owner() {
        return Ok(());
    }
    if &applied.actor != actor {
        return Err(UndoError::NotOwnEvent);
    }
    let inverse = crate::undo::inverse_of(state, event)?;
    authorize_inverse(state, actor, event, &inverse)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::NewTask;
    use crate::ids::TaskId;

    fn lid(n: u8) -> ListId {
        ListId::new_v7(1, [n; 10])
    }

    #[test]
    fn list_beats_collection_beats_default() {
        let (c, l) = (CollectionId::new_v7(1, [1; 10]), lid(2));
        let mut g = Grants::with_default(AccessLevel::Read);
        assert_eq!(g.level_for_list(l, Some(c)), AccessLevel::Read);
        g.collections.insert(c, AccessLevel::Capture);
        assert_eq!(g.level_for_list(l, Some(c)), AccessLevel::Capture);
        g.lists.insert(l, AccessLevel::None);
        assert_eq!(g.level_for_list(l, Some(c)), AccessLevel::None);
        assert_eq!(g.max_level(), AccessLevel::Capture);
    }

    #[test]
    fn levels_are_ordered() {
        assert!(AccessLevel::None < AccessLevel::Read);
        assert!(AccessLevel::Read < AccessLevel::Capture);
        assert!(AccessLevel::Capture < AccessLevel::Plan);
    }

    #[test]
    fn capture_creates_but_cannot_plan() {
        let mut st = State::new();
        let agent = Actor::agent("bot").unwrap();
        let list = lid(1);
        let mut g = Grants::default();
        g.lists.insert(list, AccessLevel::Capture);
        st.grants.insert(AgentId::new("bot").unwrap(), g);
        let task = TaskId::new_v7(1, [3; 10]);
        let create = Command::CreateTask(Box::new(NewTask::new(task, list, "x")));
        assert!(authorize(&st, &agent, &create).is_ok());
        let complete = Command::CompleteTask { id: task };
        let err = authorize(&st, &agent, &complete).unwrap_err();
        assert_eq!(err.have, AccessLevel::None); // task unknown -> default level
        let tagged = {
            let mut n = NewTask::new(task, list, "x");
            n.my_day = Some(crate::time::Date::constant(2026, 1, 1));
            Command::CreateTask(Box::new(n))
        };
        assert!(authorize(&st, &agent, &tagged).is_err());
    }

    #[test]
    fn creating_a_tag_needs_plan_somewhere_and_read_globally() {
        use crate::ids::TagId;
        let mut st = State::new();
        let agent = Actor::agent("bot").unwrap();
        let mk = Command::CreateTag {
            id: TagId::new_v7(1, [1; 10]),
            name: "t".into(),
            color: None,
        };
        let rename = Command::UpdateTag {
            id: TagId::new_v7(1, [1; 10]),
            name: Some("u".into()),
            color: None,
        };
        let denied = |st: &State, cmd: &Command| authorize(st, &agent, cmd).unwrap_err();
        let bot_id = AgentId::new("bot").unwrap();
        // Plan on one list, nothing by default: read is what's missing.
        let mut g = Grants::with_default(AccessLevel::None);
        g.lists.insert(lid(1), AccessLevel::Plan);
        st.grants.insert(bot_id.clone(), g.clone());
        let e = denied(&st, &mk);
        assert_eq!(
            (e.needed, e.have, e.owner_only),
            (AccessLevel::Read, AccessLevel::None, false)
        );
        // Read by default, but plan nowhere.
        st.grants
            .insert(bot_id.clone(), Grants::with_default(AccessLevel::Read));
        let e = denied(&st, &mk);
        assert_eq!((e.needed, e.have), (AccessLevel::Plan, AccessLevel::Read));
        // Both: allowed.
        g.default = AccessLevel::Read;
        st.grants.insert(bot_id.clone(), g);
        assert!(authorize(&st, &agent, &mk).is_ok());
        // Renaming stays the owner's, even with plan everywhere.
        st.grants
            .insert(bot_id, Grants::with_default(AccessLevel::Plan));
        assert!(authorize(&st, &agent, &mk).is_ok());
        assert!(denied(&st, &rename).owner_only);
        assert!(authorize(&st, &Actor::owner("pc").unwrap(), &rename).is_ok());
    }

    #[test]
    fn unknown_agent_has_nothing_and_structure_is_owner_only() {
        let st = State::new();
        let agent = Actor::agent("ghost").unwrap();
        let cmd = Command::CreateTask(Box::new(NewTask::new(
            TaskId::new_v7(1, [1; 10]),
            lid(1),
            "x",
        )));
        assert!(authorize(&st, &agent, &cmd).is_err());
        let mk = Command::CreateCollection {
            id: CollectionId::new_v7(1, [1; 10]),
            name: "c".into(),
            color: None,
            placement: Default::default(),
        };
        assert!(authorize(&st, &Actor::owner("pc").unwrap(), &mk).is_ok());
        assert!(authorize(&st, &Actor::Importer, &mk).is_ok());
        assert!(authorize(&st, &Actor::EmailIngest, &mk).is_err());
    }

    #[test]
    fn import_is_for_the_owner_alone() {
        let mut st = State::new();
        let list = crate::model::List {
            id: lid(1),
            name: "l".into(),
            collection: None,
            color: None,
            position: crate::ordering::PosKey::new("V").unwrap(),
            created: crate::model::Stamp {
                at: crate::time::Timestamp::UNIX_EPOCH,
                by: Actor::System,
            },
            deleted: None,
        };
        let cmd = Command::ImportEntity {
            entity: Box::new(crate::model::Entity::List(list)),
        };
        assert!(authorize(&st, &Actor::owner("pc").unwrap(), &cmd).is_ok());
        for actor in [
            Actor::agent("bot").unwrap(),
            Actor::Importer,
            Actor::System,
            Actor::EmailIngest,
        ] {
            let err = authorize(&st, &actor, &cmd).unwrap_err();
            assert!(err.owner_only, "{actor:?}");
        }
        // Even an agent with Plan everywhere is refused.
        st.grants.insert(
            crate::actor::AgentId::new("bot").unwrap(),
            Grants::with_default(AccessLevel::Plan),
        );
        assert!(authorize(&st, &Actor::agent("bot").unwrap(), &cmd).is_err());
    }
}
