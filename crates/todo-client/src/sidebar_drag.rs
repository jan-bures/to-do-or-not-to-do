//! Shared drop planning for real sidebar rows. Smart rooms cannot be sources or targets.

use todo_core::command::Placement;
use todo_core::ids::{CollectionId, EntityId, ListId};
use todo_core::state::Scope;
use todo_core::{Command, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    List(ListId),
    Collection(CollectionId),
}

impl Row {
    pub fn entity(self) -> EntityId {
        match self {
            Self::List(id) => id.into(),
            Self::Collection(id) => id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Before(Row),
    After(Row),
    Into(CollectionId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drag {
    pub source: Row,
    scope: Scope,
}

impl Drag {
    pub fn start(state: &State, source: Row) -> Option<Self> {
        let entity = source.entity();
        state.is_live(entity).then_some(Self {
            source,
            scope: state.scope_of(entity)?,
        })
    }

    /// Do not move a row again if it was deleted or changed groups during the gesture.
    pub fn current(self, state: &State) -> bool {
        Self::start(state, self.source) == Some(self)
    }

    /// Interpret a hit on a visible row. A collection below its header includes its children:
    /// collection drags target the boundary of the entire group, never a gap inside it.
    pub fn hit(self, state: &State, row: Row, after: bool) -> Option<Target> {
        let target = match (self.source, row) {
            (Row::List(_), Row::Collection(id)) => Target::Into(id),
            (Row::Collection(_), Row::List(id)) => {
                Target::After(Row::Collection(state.lists.get(&id)?.collection?))
            }
            (_, _) if after => Target::After(row),
            _ => Target::Before(row),
        };
        self.command(state, target).map(|_| target)
    }

    pub fn command(self, state: &State, target: Target) -> Option<Command> {
        if !self.current(state) {
            return None;
        }
        if let Target::Into(collection) = target {
            let Row::List(id) = self.source else {
                return None;
            };
            return crate::intent::organize::move_list(state, id, Some(collection));
        }
        let (anchor, after) = match target {
            Target::Before(row) => (row.entity(), false),
            Target::After(row) => (row.entity(), true),
            Target::Into(_) => return None,
        };
        let source = self.source.entity();
        if anchor == source || !state.is_live(anchor) {
            return None;
        }
        let scope = state.scope_of(anchor)?;
        let placement = if after {
            Placement::After(anchor)
        } else {
            Placement::Before(anchor)
        };
        if scope == self.scope {
            // Two visual sides can refer to the same gap. Compare the live sibling order,
            // not fractional position keys, so a drop back into that gap submits nothing.
            let siblings: Vec<_> = state
                .siblings(scope)
                .into_iter()
                .map(|(_, id)| id)
                .filter(|&id| state.is_live(id))
                .collect();
            let from = siblings.iter().position(|&id| id == source)?;
            let at = siblings.iter().position(|&id| id == anchor)?;
            let gap = at + usize::from(after);
            if gap == from || gap == from + 1 {
                return None;
            }
            return Some(Command::Reorder {
                id: source,
                placement,
            });
        }
        match (self.source, scope) {
            (Row::List(id), Scope::Lists(collection)) => Some(Command::MoveList {
                id,
                collection,
                placement,
            }),
            _ => None,
        }
    }
}

pub fn label(command: &Command) -> &'static str {
    match command {
        Command::MoveList { .. } => "Moved list",
        Command::Reorder {
            id: EntityId::Collection(_),
            ..
        } => "Reordered collection",
        _ => "Reordered list",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_slots_follow_live_siblings_and_collection_boundaries() {
        use todo_core::{Actor, apply, decide, time::Timestamp};
        use uuid::Uuid;
        let mut state = State::new();
        let [first, second] = [1, 2].map(|n| CollectionId::from_uuid(Uuid::from_u128(n)));
        let [a, b, c, d, loose] = [3, 4, 5, 6, 7].map(|n| ListId::from_uuid(Uuid::from_u128(n)));
        let mut run = |command| {
            for event in decide(
                &state,
                &Actor::owner("test").unwrap(),
                &command,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap()
            {
                apply(&mut state, &event).unwrap();
            }
        };
        for id in [first, second] {
            run(crate::intent::organize::create_collection(id, "Collection"));
        }
        for (id, group) in [
            (a, Some(first)),
            (b, Some(first)),
            (c, Some(first)),
            (d, Some(second)),
            (loose, None),
        ] {
            run(crate::intent::create_list(id, group, "List"));
        }
        let pick = |id| Drag::start(&state, Row::List(id)).expect("live list");
        let before = |id| Target::Before(Row::List(id));
        let after = |id| Target::After(Row::List(id));
        assert_eq!(
            pick(b).command(&state, before(a)),
            Some(Command::Reorder {
                id: b.into(),
                placement: Placement::Before(a.into()),
            })
        );
        assert_eq!(
            pick(b).command(&state, after(c)),
            Some(Command::Reorder {
                id: b.into(),
                placement: Placement::After(c.into()),
            })
        );
        for own_gap in [
            after(a),
            before(b),
            after(b),
            before(c),
            Target::Into(first),
        ] {
            assert_eq!(pick(b).command(&state, own_gap), None);
        }
        for (target, collection, placement) in [
            (before(d), Some(second), Placement::Before(d.into())),
            (after(d), Some(second), Placement::After(d.into())),
            (Target::Into(second), Some(second), Placement::Last), // also when collapsed
            (before(loose), None, Placement::Before(loose.into())),
            (after(loose), None, Placement::After(loose.into())),
        ] {
            assert_eq!(
                pick(b).command(&state, target),
                Some(Command::MoveList {
                    id: b,
                    collection,
                    placement
                })
            );
        }
        let collection = Drag::start(&state, Row::Collection(first)).expect("live collection");
        assert_eq!(
            collection.command(&state, Target::After(Row::Collection(second))),
            Some(Command::Reorder {
                id: first.into(),
                placement: Placement::After(second.into()),
            })
        );
        for target in [
            Target::Before(Row::Collection(second)),
            Target::Into(second),
            before(loose),
        ] {
            assert_eq!(collection.command(&state, target), None);
        }
    }
}
