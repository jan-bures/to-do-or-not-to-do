//! Event scopes: where an event happened, recorded when it was written.
//!
//! An event's visibility to an agent cannot be decided against the current state alone: a task's notes
//! edited in a private list stay in the log after the task moves to a list the agent can read. So every
//! event is stored with its **scope**: each list (with the collection it was in at that time),
//! collection, or global area the event touched, resolved against the state just before the event and
//! just after it. Both ends of a move are included, and a list moved between collections is recorded
//! with both collections.
//!
//! An agent sees an event only if it can read every entry of the scope under its current grants
//! ([`readable`]) and the event's targets in the current state (see
//! [`crate::api::visibility::event_visible`]).
//!
//! Scopes live in the `event_scopes` table, written in the same transaction as the event. Logs written
//! before the table existed are backfilled once at startup by folding the log
//! ([`crate::db::backfill_scopes`]).
//!
//! An event is also checked against where its entity is **now** ([`location`]). An entity that is no
//! longer in the state (its creation was undone) has a last known location instead
//! ([`Whereabouts`]), kept by the engine and rebuilt from the log at startup.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use todo_core::access::{Target, entity_target, undo_requirements};
use todo_core::event::EventPayload;
use todo_core::ids::{CollectionId, EntityId, ListId};
use todo_core::{AccessLevel, Actor, State};

/// One place an event touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum ScopeEntry {
    /// Not tied to a list or collection (tags).
    Global,
    /// A collection.
    Collection {
        /// The collection.
        c: CollectionId,
    },
    /// A list, with the collection it was in.
    List {
        /// The list.
        l: ListId,
        /// Its collection at the time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<CollectionId>,
    },
}

/// Everything an event touched.
pub type EventScope = BTreeSet<ScopeEntry>;

/// The last known location of entities that are no longer in the state.
pub type Whereabouts = BTreeMap<EntityId, ScopeEntry>;

/// Where `id` is in `state`: its list (with that list's collection), its collection, or global for a
/// tag. `None` if it is not in the state (or hangs off something that is not).
#[must_use]
pub fn location(state: &State, id: EntityId) -> Option<ScopeEntry> {
    match id {
        EntityId::Tag(t) => state.tags.contains_key(&t).then_some(ScopeEntry::Global),
        EntityId::Collection(c) => state
            .collections
            .contains_key(&c)
            .then_some(ScopeEntry::Collection { c }),
        _ => match entity_target(state, id) {
            Target::List(l) => state.lists.get(&l).map(|list| ScopeEntry::List {
                l,
                c: list.collection,
            }),
            Target::Collection(c) => Some(ScopeEntry::Collection { c }),
            Target::Global => None,
        },
    }
}

/// Where `id` is now: in `state`, or else where it was last seen.
#[must_use]
pub fn current_location(state: &State, gone: &Whereabouts, id: EntityId) -> Option<ScopeEntry> {
    location(state, id).or_else(|| gone.get(&id).copied())
}

/// The change to [`Whereabouts`] after an event about `id`, which was at `before` and is now in
/// `after` (or not): `Some(Some(loc))` to record a last location, `Some(None)` to forget one.
#[must_use]
pub fn whereabouts_change(
    before: Option<ScopeEntry>,
    after: &State,
    id: EntityId,
) -> Option<Option<ScopeEntry>> {
    match (before, location(after, id)) {
        (Some(was), None) => Some(Some(was)),
        (_, Some(_)) => Some(None),
        (None, None) => None,
    }
}

/// Applies a [`whereabouts_change`].
pub fn record_whereabouts(
    gone: &mut Whereabouts,
    id: EntityId,
    change: Option<Option<ScopeEntry>>,
) {
    match change {
        Some(Some(loc)) => {
            gone.insert(id, loc);
        }
        Some(None) => {
            gone.remove(&id);
        }
        None => {}
    }
}

/// Collects an event's scope across the state before and after it.
#[derive(Debug, Default)]
pub struct ScopeBuilder {
    scope: EventScope,
    unresolved: BTreeSet<ListId>,
}

impl ScopeBuilder {
    /// Adds the places `payload` touches in `state` (call once before applying and once after).
    pub fn observe(&mut self, state: &State, payload: &EventPayload) {
        // Where the entity actually is (for a withdrawal, where it was taken from).
        if let Some(here) = location(state, payload.target()) {
            self.scope.insert(here);
        }
        for (_, target) in undo_requirements(state, payload) {
            match target {
                Target::Global => {
                    self.scope.insert(ScopeEntry::Global);
                }
                Target::Collection(c) => {
                    self.scope.insert(ScopeEntry::Collection { c });
                }
                Target::List(l) => match state.lists.get(&l) {
                    Some(list) => {
                        self.scope.insert(ScopeEntry::List {
                            l,
                            c: list.collection,
                        });
                    }
                    // Not in this state (created or withdrawn by the event): the other side has it.
                    None => {
                        self.unresolved.insert(l);
                    }
                },
            }
        }
    }

    /// The scope. A list that resolved in neither state is recorded without a collection.
    #[must_use]
    pub fn finish(mut self) -> EventScope {
        for l in self.unresolved {
            let known = self
                .scope
                .iter()
                .any(|e| matches!(e, ScopeEntry::List { l: x, .. } if *x == l));
            if !known {
                self.scope.insert(ScopeEntry::List { l, c: None });
            }
        }
        self.scope
    }
}

/// True if `actor` may read every place in `scope`, under its current grants. Non-agents read
/// everything; an agent without grants reads nothing; an empty scope is not readable by agents.
#[must_use]
pub fn readable(state: &State, actor: &Actor, scope: &EventScope) -> bool {
    let Actor::Agent { agent_id } = actor else {
        return true;
    };
    let Some(g) = state.grants.get(agent_id) else {
        return false;
    };
    !scope.is_empty()
        && scope.iter().all(|e| {
            let level = match *e {
                ScopeEntry::Global => g.default,
                ScopeEntry::Collection { c } => g.level_for_collection(c),
                ScopeEntry::List { l, c } => g.level_for_list(l, c),
            };
            level >= AccessLevel::Read
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::Grants;
    use todo_core::time::Timestamp;
    use todo_core::{AgentId, apply, decide};

    fn run(state: &mut State, cmd: serde_json::Value) -> Vec<EventScope> {
        let owner = Actor::owner("pc").unwrap();
        let cmd = serde_json::from_value(cmd).unwrap();
        let events = decide(
            state,
            &owner,
            &cmd,
            Timestamp::from_second(1_700_000_000).unwrap(),
        )
        .unwrap();
        let mut out = Vec::new();
        for e in &events {
            let mut b = ScopeBuilder::default();
            b.observe(state, &e.payload);
            apply(state, e).unwrap();
            b.observe(state, &e.payload);
            out.push(b.finish());
        }
        out
    }

    #[test]
    fn moves_record_both_ends_and_lists_their_collection() {
        let mut s = State::new();
        let (c1, c2) = (
            CollectionId::new_v7(1, [1; 10]),
            CollectionId::new_v7(1, [2; 10]),
        );
        let (l1, l2) = (ListId::new_v7(1, [3; 10]), ListId::new_v7(1, [4; 10]));
        let task = todo_core::ids::TaskId::new_v7(1, [5; 10]);
        for c in [c1, c2] {
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_collection", "id": c, "name": "c" }),
            );
        }
        let created = run(
            &mut s,
            serde_json::json!({ "cmd": "create_list", "id": l1, "name": "a", "collection": c1 }),
        );
        assert_eq!(
            created[0],
            EventScope::from([ScopeEntry::List { l: l1, c: Some(c1) }])
        );
        run(
            &mut s,
            serde_json::json!({ "cmd": "create_list", "id": l2, "name": "b" }),
        );
        run(
            &mut s,
            serde_json::json!({ "cmd": "create_task", "id": task, "list": l1, "title": "t" }),
        );
        let moved = run(
            &mut s,
            serde_json::json!({ "cmd": "move_task", "id": task, "list": l2 }),
        );
        assert!(moved[0].contains(&ScopeEntry::List { l: l1, c: Some(c1) }));
        assert!(moved[0].contains(&ScopeEntry::List { l: l2, c: None }));
        let list_moved = run(
            &mut s,
            serde_json::json!({ "cmd": "move_list", "id": l1, "collection": c2 }),
        );
        assert!(list_moved[0].contains(&ScopeEntry::List { l: l1, c: Some(c1) }));
        assert!(list_moved[0].contains(&ScopeEntry::List { l: l1, c: Some(c2) }));

        let bot = Actor::agent("bot").unwrap();
        let mut g = Grants::default();
        g.collections.insert(c2, AccessLevel::Read);
        s.grants.insert(AgentId::new("bot").unwrap(), g);
        // The list is in c2 now, but was in c1 (unreadable) when these happened.
        assert!(!readable(&s, &bot, &created[0]));
        assert!(!readable(&s, &bot, &list_moved[0]));
        assert!(readable(
            &s,
            &bot,
            &EventScope::from([ScopeEntry::List { l: l1, c: Some(c2) }])
        ));
        assert!(!readable(&s, &bot, &EventScope::new()));
        assert!(readable(
            &s,
            &Actor::owner("pc").unwrap(),
            &EventScope::new()
        ));
    }

    #[test]
    fn scopes_round_trip_compactly() {
        let l = ListId::new_v7(1, [3; 10]);
        let scope = EventScope::from([ScopeEntry::Global, ScopeEntry::List { l, c: None }]);
        let json = serde_json::to_string(&scope).unwrap();
        assert!(!json.contains("null"), "{json}");
        let back: EventScope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scope);
    }
}
