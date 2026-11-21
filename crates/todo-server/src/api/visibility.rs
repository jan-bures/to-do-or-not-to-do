//! What a caller may read.
//!
//! The owner (and the other non-agent actors) read everything. An agent reads a list, and the tasks,
//! steps, links, attachments and reminders in it, when its grants give it at least
//! [`AccessLevel::Read`] there (`todo-core`'s list, then collection, then default rule). A collection
//! is visible when the agent can read it or any list in it. Tags are visible when the agent's default
//! level is at least `Read`, or when they are on a task it can read.
//!
//! Events are visible when the agent can read **every** list or collection the event touched, both
//! where it happened (its [scope](crate::scope), recorded when it was written, with each list's
//! collection at that time) and where those entities are now (for a move or a merge, both ends). So
//! notes written in a private list stay hidden after the task moves to a readable one. An event about
//! an entity that no longer resolves to a list (its creation was undone), or without a recorded
//! scope, is hidden from agents.

use crate::scope::{EventScope, readable};
use std::collections::BTreeSet;
use todo_core::access::{Target, entity_target, level_on, undo_requirements};
use todo_core::event::{Event, EventPayload};
use todo_core::ids::{CollectionId, EntityId, ListId, TagId};
use todo_core::model::{Collection, List, Tag, Task};
use todo_core::{AccessLevel, Actor, State};

/// True if `actor` may read `target`.
#[must_use]
pub fn can_read(state: &State, actor: &Actor, target: Target) -> bool {
    level_on(state, actor, target) >= AccessLevel::Read
}

/// True if the actor sees everything (no filtering needed).
#[must_use]
pub fn sees_all(actor: &Actor) -> bool {
    !actor.is_agent()
}

/// True if `actor` may read the list.
#[must_use]
pub fn list_visible(state: &State, actor: &Actor, list: ListId) -> bool {
    can_read(state, actor, Target::List(list))
}

/// True if `actor` may read the task (the list it is in).
#[must_use]
pub fn task_visible(state: &State, actor: &Actor, task: &Task) -> bool {
    list_visible(state, actor, task.list)
}

/// The collections an agent may see: those it can read and those holding a list it can read.
#[must_use]
pub fn visible_collections(state: &State, actor: &Actor) -> Option<BTreeSet<CollectionId>> {
    if sees_all(actor) {
        return None;
    }
    let mut out: BTreeSet<CollectionId> = state
        .collections
        .keys()
        .copied()
        .filter(|c| can_read(state, actor, Target::Collection(*c)))
        .collect();
    for l in state.lists.values() {
        if let Some(c) = l.collection
            && list_visible(state, actor, l.id)
        {
            out.insert(c);
        }
    }
    Some(out)
}

/// True if `c` is in `visible` (or everything is visible).
#[must_use]
pub fn collection_in(visible: Option<&BTreeSet<CollectionId>>, c: &Collection) -> bool {
    visible.is_none_or(|v| v.contains(&c.id))
}

/// True if `actor` may read the list `l`.
#[must_use]
pub fn list_entity_visible(state: &State, actor: &Actor, l: &List) -> bool {
    list_visible(state, actor, l.id)
}

/// The tags an agent may see (`None`: all).
#[must_use]
pub fn visible_tags(state: &State, actor: &Actor) -> Option<BTreeSet<TagId>> {
    if sees_all(actor) || can_read(state, actor, Target::Global) {
        return None;
    }
    let mut out = BTreeSet::new();
    for t in state.tasks.values() {
        if task_visible(state, actor, t) {
            out.extend(t.tags.iter().copied());
        }
    }
    Some(out)
}

/// True if `t` is in `visible` (or everything is visible).
#[must_use]
pub fn tag_in(visible: Option<&BTreeSet<TagId>>, t: &Tag) -> bool {
    visible.is_none_or(|v| v.contains(&t.id))
}

/// True if `actor` may see `event`, which was written with `scope` (see the module docs).
#[must_use]
pub fn event_visible(
    state: &State,
    actor: &Actor,
    event: &Event,
    scope: Option<&EventScope>,
) -> bool {
    if sees_all(actor) {
        return true;
    }
    if !scope.is_some_and(|s| readable(state, actor, s)) {
        return false;
    }
    let target = event.payload.target();
    let carries_entity = matches!(
        event.payload,
        EventPayload::Created { .. } | EventPayload::Withdrawn { .. }
    );
    if !carries_entity
        && !matches!(target, EntityId::Tag(_))
        && entity_target(state, target) == Target::Global
    {
        return false;
    }
    undo_requirements(state, &event.payload)
        .into_iter()
        .all(|(_, t)| can_read(state, actor, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::Grants;
    use todo_core::command::{Command, NewTask};
    use todo_core::ids::TaskId;
    use todo_core::time::Timestamp;
    use todo_core::{AgentId, apply, decide};

    fn run(state: &mut State, cmd: Command) -> Vec<(Event, EventScope)> {
        let owner = Actor::owner("pc").unwrap();
        let events = decide(
            state,
            &owner,
            &cmd,
            Timestamp::from_second(1_700_000_000).unwrap(),
        )
        .unwrap();
        events
            .into_iter()
            .map(|e| {
                let mut b = crate::scope::ScopeBuilder::default();
                b.observe(state, &e.payload);
                apply(state, &e).unwrap();
                b.observe(state, &e.payload);
                (e, b.finish())
            })
            .collect()
    }

    fn sees(s: &State, a: &Actor, e: &(Event, EventScope)) -> bool {
        event_visible(s, a, &e.0, Some(&e.1))
    }

    fn list(n: u8) -> ListId {
        ListId::new_v7(1, [n; 10])
    }

    #[test]
    fn agents_see_only_lists_they_can_read() {
        let mut s = State::new();
        let (open, hidden) = (list(1), list(2));
        for (id, name) in [(open, "open"), (hidden, "hidden")] {
            run(
                &mut s,
                serde_json::from_value(serde_json::json!({
                    "cmd": "create_list", "id": id, "name": name
                }))
                .unwrap(),
            );
        }
        let t_open = TaskId::new_v7(1, [3; 10]);
        let t_hidden = TaskId::new_v7(1, [4; 10]);
        let e_open = run(
            &mut s,
            Command::CreateTask(Box::new(NewTask::new(t_open, open, "a"))),
        );
        let e_hidden = run(
            &mut s,
            Command::CreateTask(Box::new(NewTask::new(t_hidden, hidden, "b"))),
        );
        let moved = run(
            &mut s,
            serde_json::from_value(serde_json::json!({
                "cmd": "move_task", "id": t_open, "list": hidden
            }))
            .unwrap(),
        );
        let bot = Actor::agent("bot").unwrap();
        let mut g = Grants::default();
        g.lists.insert(open, AccessLevel::Read);
        s.grants.insert(AgentId::new("bot").unwrap(), g);
        assert!(list_visible(&s, &bot, open));
        assert!(!list_visible(&s, &bot, hidden));
        assert!(!sees(&s, &bot, &e_hidden[0]));
        assert!(
            !sees(&s, &bot, &moved[0]),
            "a move into a hidden list is hidden"
        );
        // Its creation in the open list stays visible: the event carries the list it was made in.
        assert!(sees(&s, &bot, &e_open[0]));
        // Without a recorded scope, agents see nothing.
        assert!(!event_visible(&s, &bot, &e_open[0].0, None));
        assert!(visible_tags(&s, &bot).unwrap().is_empty());
        let owner = Actor::owner("pc").unwrap();
        assert!(sees(&s, &owner, &e_hidden[0]));
        assert!(visible_collections(&s, &owner).is_none());
    }
}
