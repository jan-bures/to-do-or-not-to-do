//! In-memory history of explicit room, view and detail navigation. Selection, including
//! cursor-driven detail previews, belongs to the client UI, not to a history entry.

use std::collections::VecDeque;

use todo_core::{State, ids::TaskId, model::ListView};

use crate::room_header::RoomKey;

pub const LIMIT: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location<R> {
    pub room: R,
    pub view: ListView,
    pub task: Option<TaskId>,
}

pub fn room_exists(state: &State, room: RoomKey<'_>) -> bool {
    match room {
        RoomKey::List(id) => state.is_live(id.into()),
        RoomKey::Collection(id) => state.is_live(id.into()),
        RoomKey::Smart(_) | RoomKey::Activity(_) | RoomKey::Trash => true,
    }
}

pub fn view_exists(state: &State, room: RoomKey<'_>, view: ListView) -> bool {
    match view {
        ListView::List => true,
        ListView::Board => {
            matches!(room, RoomKey::List(id) if state.lists.get(&id).is_some_and(|list| list.features.workflow))
        }
        ListView::Timeline => crate::timeline::available(state, room),
    }
}

pub fn task_exists(state: &State, task: TaskId) -> bool {
    state
        .tasks
        .get(&task)
        .is_some_and(|t| crate::intent::organize::is_actionable(state, t))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

impl Direction {
    fn step(self, position: usize) -> Option<usize> {
        match self {
            Self::Back => position.checked_sub(1),
            Self::Forward => position.checked_add(1),
        }
    }
}

/// A bounded sequence and a cursor. Positions stay stable as the oldest entries age out,
/// so the web adapter can associate them with native browser history entries.
#[derive(Debug)]
pub struct History<L> {
    entries: VecDeque<L>,
    first: usize,
    position: usize,
}

impl<L: PartialEq> History<L> {
    pub fn new(initial: L) -> Self {
        Self {
            entries: VecDeque::from([initial]),
            first: 0,
            position: 0,
        }
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn get(&self, position: usize) -> Option<&L> {
        self.entries.get(position.checked_sub(self.first)?)
    }

    pub fn current(&self) -> &L {
        &self.entries[self.position - self.first]
    }

    /// True if a new entry was appended. A duplicate doesn't discard the forward entries.
    pub fn push(&mut self, entry: L) -> bool {
        if self.current() == &entry {
            return false;
        }
        self.position += 1;
        self.entries.truncate(self.position - self.first);
        self.entries.push_back(entry);
        if self.entries.len() > LIMIT {
            self.entries.pop_front();
            self.first += 1;
        }
        true
    }

    /// Updates the initial entry when a client learns its resolved location (for example, after
    /// loading the default view for a deep link). Other entries keep their original location.
    pub fn resolve_initial(&mut self, entry: L) {
        if self.first == 0 && self.entries.len() == 1 {
            self.entries[0] = entry;
        }
    }

    pub fn target(&self, direction: Direction, available: impl FnMut(&L) -> bool) -> Option<usize> {
        self.find_from(direction.step(self.position)?, direction, available)
    }

    /// Finds a restorable entry at or beyond a native history landing position. A deleted room/task
    /// is not rewritten or removed: the position still belongs to the browser, and Undo can make
    /// it usable again.
    pub fn find_from(
        &self,
        mut position: usize,
        direction: Direction,
        mut available: impl FnMut(&L) -> bool,
    ) -> Option<usize> {
        loop {
            let entry = self.get(position)?;
            if available(entry) {
                return Some(position);
            }
            position = direction.step(position)?;
        }
    }

    pub fn visit(&mut self, position: usize) -> Option<&L> {
        self.get(position)?;
        self.position = position;
        Some(self.current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn skips_missing_locations_in_both_directions_and_can_restore_them_later() {
        let mut history = History::new(1);
        for value in [2, 3, 4] {
            assert!(history.push(value));
        }
        let live = |value: &u8| !matches!(*value, 2 | 3);
        assert_eq!(history.find_from(2, Direction::Back, live), Some(0));
        assert_eq!(history.visit(0), Some(&1));
        assert_eq!(history.target(Direction::Forward, live), Some(3));
        assert_eq!(history.target(Direction::Forward, |_| true), Some(1));
        assert_eq!(history.target(Direction::Back, live), None);
    }

    #[test]
    fn pushes_branch_without_renumbering_retained_entries() {
        let mut history = History::new(0);
        for i in 1..=LIMIT + 5 {
            history.push(i);
        }
        assert_eq!(history.entries.len(), LIMIT);
        assert_eq!(history.get(5), None);
        assert_eq!(history.get(6), Some(&6));
        assert_eq!(history.visit(LIMIT + 1), Some(&(LIMIT + 1)));
        assert!(!history.push(LIMIT + 1));
        assert!(history.target(Direction::Forward, |_| true).is_some());
        assert!(history.push(0));
        assert_eq!(history.position(), LIMIT + 2);
        assert_eq!(history.target(Direction::Forward, |_| true), None);
    }

    proptest! {
        #[test]
        fn arbitrary_navigation_keeps_history_invariants(
            actions in prop::collection::vec((0..5u8, 0..12u8), 0..420)
        ) {
            let mut history = History::new(0);
            for (action, value) in actions {
                let old = *history.current();
                let at = history.position();
                match action {
                    0..=2 => {
                        prop_assert_eq!(history.push(value), old != value);
                        prop_assert_eq!(*history.current(), value);
                        if old != value {
                            prop_assert_eq!(history.target(Direction::Forward, |_| true), None);
                        }
                    }
                    n => {
                        let (direction, reverse) = if n == 3 {
                            (Direction::Back, Direction::Forward)
                        } else {
                            (Direction::Forward, Direction::Back)
                        };
                        if let Some(next) = history.target(direction, |_| true) {
                            history.visit(next);
                            prop_assert_eq!(history.target(reverse, |_| true), Some(at));
                            prop_assert_eq!(history.visit(at), Some(&old));
                            history.visit(next);
                        }
                    }
                }
                prop_assert!((1..=LIMIT).contains(&history.entries.len()));
                prop_assert!(history.position >= history.first);
                prop_assert!(history.position < history.first + history.entries.len());
                prop_assert!(history.entries.iter().zip(history.entries.iter().skip(1)).all(|(a, b)| a != b));
            }
        }
    }
}
