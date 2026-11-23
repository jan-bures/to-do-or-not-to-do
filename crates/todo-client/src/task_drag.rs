//! The same task drag and keyboard nudge semantics on every surface. No I/O.

use std::collections::BTreeSet;

use todo_core::command::Placement;
use todo_core::ids::{ListId, TaskId};
use todo_core::time::Date;
use todo_core::{Command, State};

use todo_core::decide::MAX_BULK_TASKS;

use crate::intent::organize::confirm::one_line;
use crate::selection::{self, BulkLimit, BulkTaskAction};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drag {
    pub source: TaskId,
    members: Vec<(TaskId, ListId)>,
}

/// A visual gap before or after a row. The row must still be visible when the drop lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub row: TaskId,
    pub after: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    MyDay,
    List(ListId),
}

impl Drag {
    /// `selected` is in visible room order, not click order. Starting on an unselected row lifts
    /// just that row; starting on any selected row lifts the whole selection.
    pub fn start(state: &State, source: TaskId, selected: &[TaskId]) -> Option<Self> {
        let ids = if selected.contains(&source) {
            selected
        } else {
            std::slice::from_ref(&source)
        };
        let mut seen = BTreeSet::new();
        let members = ids
            .iter()
            .filter(|&&id| seen.insert(id))
            .map(|&id| {
                let task = state.tasks.get(&id)?;
                state.is_live(id.into()).then_some((id, task.list))
            })
            .collect::<Option<_>>()?;
        Some(Self { source, members })
    }

    pub fn ids(&self) -> impl ExactSizeIterator<Item = TaskId> + '_ {
        self.members.iter().map(|&(id, _)| id)
    }

    pub fn current(&self, state: &State) -> bool {
        self.members.iter().all(|&(id, list)| {
            state.is_live(id.into()) && state.tasks.get(&id).is_some_and(|task| task.list == list)
        })
    }

    /// Only real lists and My Day are destinations. Highlight only when the drop would change
    /// something, including for a mixed-list group that's partly in the destination already.
    pub fn eligible(&self, state: &State, dest: Destination, today: Date) -> bool {
        if !self.current(state) {
            return false;
        }
        match dest {
            Destination::List(list) => {
                state.is_live(list.into()) && self.members.iter().any(|&(_, from)| from != list)
            }
            Destination::MyDay => self.ids().any(|id| state.tasks[&id].my_day != Some(today)),
        }
    }

    pub fn drop_on(
        &self,
        state: &State,
        dest: Destination,
        today: Date,
    ) -> Result<Option<(Command, String)>, BulkLimit> {
        if !self.eligible(state, dest, today) {
            return Ok(None);
        }
        let action = match dest {
            Destination::List(list) => BulkTaskAction::Move { list },
            Destination::MyDay => BulkTaskAction::MyDay {
                day: today,
                on: true,
            },
        };
        let ids: Vec<_> = self.ids().collect();
        let (cmd, n) = if ids.len() == 1 {
            let Some(cmd) = action.change(&state.tasks[&ids[0]]) else {
                return Ok(None);
            };
            (cmd, 1)
        } else {
            let Some((cmd, _)) = selection::batch(state, &ids, action)? else {
                return Ok(None);
            };
            let Command::BulkTasks { tasks, .. } = &cmd else {
                return Ok(None);
            };
            let n = tasks.len();
            (cmd, n)
        };
        let count = tasks_label(n);
        let label = match dest {
            Destination::List(list) => {
                format!("Moved {count} to {}", one_line(&state.lists[&list].name))
            }
            Destination::MyDay => format!("Added {count} to My Day"),
        };
        Ok(Some((cmd, label)))
    }

    /// Map a visual slot to an unselected, visible sibling. Hidden and completed siblings keep
    /// their relative order; neither is used as an anchor. No change if the visible order agrees.
    /// `list` is `None` in a smart room, a collection or a non-manual sort.
    pub fn placement(
        &self,
        state: &State,
        list: Option<ListId>,
        visible: &[TaskId],
        slot: Slot,
    ) -> Option<Placement> {
        let list = list?;
        let i = visible.iter().position(|&id| id == slot.row)?;
        self.at_index(state, list, visible, i + usize::from(slot.after))
    }

    fn at_index(
        &self,
        state: &State,
        list: ListId,
        visible: &[TaskId],
        index: usize,
    ) -> Option<Placement> {
        if index > visible.len() || !self.current(state) {
            return None;
        }
        let picked: BTreeSet<_> = self.ids().collect();
        if self
            .members
            .iter()
            .any(|&(id, from)| from != list || state.tasks[&id].is_done() || !visible.contains(&id))
        {
            return None;
        }
        let (before, after) = visible.split_at(index);
        let unpicked = |id: &&TaskId| !picked.contains(id);
        let block = || visible.iter().filter(|id| picked.contains(id));
        if visible.iter().eq(before
            .iter()
            .filter(unpicked)
            .chain(block())
            .chain(after.iter().filter(unpicked)))
        {
            return None;
        }
        // Prefer the side of the actual gap that was below the pointer. At the end, instead
        // anchor to the last remaining visible task, not to the end of the unfiltered list.
        let (anchor, after) = after
            .iter()
            .find(unpicked)
            .map(|&id| (id, false))
            .or_else(|| before.iter().rfind(unpicked).map(|&id| (id, true)))?;
        let sibling = state.tasks.get(&anchor)?;
        if !state.is_live(anchor.into()) || sibling.is_done() || sibling.list != list {
            return None;
        }
        Some(if after {
            Placement::After(anchor.into())
        } else {
            Placement::Before(anchor.into())
        })
    }

    fn reorder(
        &self,
        list: ListId,
        order: &[TaskId],
        placement: Placement,
    ) -> Result<Option<(Command, String)>, BulkLimit> {
        // Re-read order at drop, so an intervening sync cannot reverse the picked block.
        let picked: BTreeSet<_> = self.ids().collect();
        let tasks: Vec<_> = order
            .iter()
            .copied()
            .filter(|id| picked.contains(id))
            .collect();
        let n = tasks.len();
        if n > MAX_BULK_TASKS {
            return Err(BulkLimit);
        }
        let cmd = if n == 1 {
            Command::Reorder {
                id: tasks[0].into(),
                placement,
            }
        } else {
            Command::BulkTasks {
                tasks,
                action: BulkTaskAction::Reorder { list, placement },
            }
        };
        Ok(Some((cmd, format!("Reordered {}", tasks_label(n)))))
    }

    pub fn drop_in(
        &self,
        state: &State,
        list: Option<ListId>,
        visible: &[TaskId],
        slot: Slot,
    ) -> Result<Option<(Command, String)>, BulkLimit> {
        let Some(list) = list else { return Ok(None) };
        let Some(placement) = self.placement(state, Some(list), visible, slot) else {
            return Ok(None);
        };
        self.reorder(list, visible, placement)
    }

    /// Nudge past the first unselected row immediately outside the selected extent.
    pub fn nudge(
        &self,
        state: &State,
        list: ListId,
        visible: &[TaskId],
        down: bool,
    ) -> Result<Option<(Command, String)>, BulkLimit> {
        let slot = if down {
            visible
                .iter()
                .rposition(|id| self.ids().any(|picked| picked == *id))
                .and_then(|i| visible.get(i + 1))
        } else {
            visible
                .iter()
                .position(|id| self.ids().any(|picked| picked == *id))
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| visible.get(i))
        }
        .map(|&row| Slot { row, after: down });
        match slot {
            Some(slot) => self.drop_in(state, Some(list), visible, slot),
            None => Ok(None),
        }
    }
}

fn tasks_label(count: usize) -> String {
    format!("{count} {}", if count == 1 { "task" } else { "tasks" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::command::NewTask;
    use todo_core::time::Timestamp;
    use todo_core::{Actor, apply, decide};
    use uuid::Uuid;

    fn scenario() -> (State, ListId, ListId, [TaskId; 6]) {
        let [list, dest] = [8, 9].map(|n| ListId::from_uuid(Uuid::from_u128(n)));
        let ids = [1, 2, 3, 4, 5, 6].map(|n| TaskId::from_uuid(Uuid::from_u128(n)));
        let mut state = State::new();
        for id in [list, dest] {
            run(
                &mut state,
                Command::CreateList {
                    id,
                    collection: None,
                    name: format!("List {}", if id == list { "A" } else { "B" }),
                    color: None,
                    placement: Placement::Last,
                },
            );
        }
        for id in ids {
            run(
                &mut state,
                Command::CreateTask(Box::new(NewTask::new(id, list, "Task"))),
            );
        }
        (state, list, dest, ids)
    }

    fn run(state: &mut State, cmd: Command) {
        for event in decide(
            state,
            &Actor::owner("test").unwrap(),
            &cmd,
            Timestamp::UNIX_EPOCH,
        )
        .unwrap()
        {
            apply(state, &event).unwrap();
        }
    }

    #[test]
    fn first_last_and_no_op_slots_ignore_the_picked_row() {
        let (state, list, _, ids) = scenario();
        let [a, b, c, _, _, f] = ids;
        let drag = Drag::start(&state, b, &[a]).unwrap();
        assert_eq!(drag.ids().collect::<Vec<_>>(), [b]);
        for (row, after, expected) in [
            (a, false, Some(Placement::Before(a.into()))),
            (f, true, Some(Placement::After(f.into()))),
            (c, false, None),
            (b, true, None),
        ] {
            assert_eq!(
                drag.placement(&state, Some(list), &ids, Slot { row, after }),
                expected
            );
        }
        assert_eq!(
            drag.placement(
                &state,
                None,
                &ids,
                Slot {
                    row: f,
                    after: true
                }
            ),
            None
        );
    }

    #[test]
    fn multi_drop_in_filtered_order_keeps_hidden_rows_out_of_the_plan() {
        let (mut state, list, _, [a, hidden_a, b, hidden_b, c, d]) = scenario();
        let open = [a, b, c, d];
        let drag = Drag::start(&state, c, &[a, c]).unwrap();
        let (cmd, toast) = drag
            .drop_in(
                &state,
                Some(list),
                &open,
                Slot {
                    row: d,
                    after: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::BulkTasks {
                tasks: vec![a, c],
                action: BulkTaskAction::Reorder {
                    list,
                    placement: Placement::After(d.into())
                }
            }
        );
        assert_eq!(toast, "Reordered 2 tasks");
        run(&mut state, cmd);
        let mut tasks: Vec<_> = state.tasks.values().collect();
        tasks.sort_by(|a, b| a.position.cmp(&b.position));
        assert_eq!(
            tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
            [hidden_a, b, hidden_b, d, a, c]
        );
        let drag = Drag::start(&state, a, &[a, c]).unwrap();
        assert!(
            drag.nudge(&state, list, &[b, d, a, c], true)
                .unwrap()
                .is_none()
        );
        let (cmd, _) = drag
            .nudge(&state, list, &[b, d, a, c], false)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::BulkTasks {
                tasks: vec![a, c],
                action: BulkTaskAction::Reorder {
                    list,
                    placement: Placement::Before(d.into())
                }
            }
        );
    }

    #[test]
    fn move_and_my_day_use_a_single_batch_and_recheck_live_members() {
        let (mut state, list, other, [a, b, ..]) = scenario();
        let today = "2026-11-21".parse().unwrap();
        let drag = Drag::start(&state, a, &[a, b]).unwrap();
        assert!(!drag.eligible(&state, Destination::List(list), today));
        let (cmd, label) = drag
            .drop_on(&state, Destination::List(other), today)
            .unwrap()
            .unwrap();
        assert_eq!(label, "Moved 2 tasks to List B");
        assert_eq!(
            cmd,
            Command::BulkTasks {
                tasks: vec![a, b],
                action: BulkTaskAction::Move { list: other }
            }
        );
        run(
            &mut state,
            Command::SetMyDay {
                id: a,
                date: Some(today),
            },
        );
        let (cmd, label) = drag
            .drop_on(&state, Destination::MyDay, today)
            .unwrap()
            .unwrap();
        assert_eq!(label, "Added 1 task to My Day");
        assert!(matches!(cmd, Command::BulkTasks { tasks, .. } if tasks == [b]));
        run(
            &mut state,
            Command::MoveTask {
                id: b,
                list: other,
                from_list: Some(list),
                placement: Placement::First,
            },
        );
        assert!(!drag.eligible(&state, Destination::MyDay, today));
    }
}
