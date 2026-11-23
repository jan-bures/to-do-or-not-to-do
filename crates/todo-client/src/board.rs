//! Pure projections and drag intents for a workflow-enabled list.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use todo_core::State;
use todo_core::command::{Command, Placement};
use todo_core::ids::{EntityId, ListId, TaskId};
use todo_core::query::{EvalContext, QueryStatus};
use todo_core::state::Scope;
use todo_core::time::days_between;

use crate::intent::organize::is_actionable;

/// The board's Done column shows completions from today and the six preceding local dates.
pub const DONE_DAYS: i64 = 7;
/// Maximum number of cards in that column, most recent first.
pub const DONE_LIMIT: usize = 20;
const STAGES: [QueryStatus; 4] = [
    QueryStatus::Backlog,
    QueryStatus::Todo,
    QueryStatus::InProgress,
    QueryStatus::Done,
];

/// One column in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub stage: QueryStatus,
    pub cards: Vec<TaskId>,
}

/// The four columns (including empty ones) and the count behind the Completed link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub columns: [Column; 4],
    pub total_done: usize,
}

fn enabled(state: &State, list: ListId) -> bool {
    state.is_live(list.into()) && state.lists.get(&list).is_some_and(|l| l.features.workflow)
}

/// Group the current cards without creating another stored sort key.
pub fn project(state: &State, list: ListId, ctx: &EvalContext) -> Option<Board> {
    if !enabled(state, list) {
        return None;
    }
    let mut columns = STAGES.map(|stage| Column {
        stage,
        cards: Vec::new(),
    });
    let mut done = BinaryHeap::with_capacity(DONE_LIMIT + 1);
    let mut total_done = 0;
    // Visit completed rows without cloning or sorting their position keys. Only recent rows go in
    // the Done ordering; the rest only contribute to the count.
    let mut open = Vec::new();
    for task in state
        .tasks
        .values()
        .filter(|t| t.list == list && is_actionable(state, t))
    {
        if let Some(at) = task.completed_at {
            total_done += 1;
            if (0..DONE_DAYS).contains(&days_between(ctx.local_date(at), ctx.today)) {
                done.push((Reverse(at), task.id));
                if done.len() > DONE_LIMIT {
                    done.pop();
                }
            }
        } else {
            open.push(task);
        }
    }
    open.sort_unstable_by_key(|t| (&t.position, t.id));
    for task in open {
        columns[QueryStatus::of(task) as usize].cards.push(task.id);
    }
    columns[QueryStatus::Done as usize].cards = done
        .into_sorted_vec()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    Some(Board {
        columns,
        total_done,
    })
}

/// Resolve a drop slot in the current destination column. `First`/`Last` refer to that column, not
/// the whole list. For Done, position is ignored: completing always uses the ordinary command.
/// Stale anchors, self-drops and moves that leave the card in its current slot return `None`.
pub fn drop_command(
    state: &State,
    list: ListId,
    card: TaskId,
    target: QueryStatus,
    slot: Placement,
) -> Option<Command> {
    let task = state.tasks.get(&card)?;
    if !enabled(state, list) || task.list != list || !is_actionable(state, task) {
        return None;
    }
    let status = match target {
        QueryStatus::Done => {
            return (!task.is_done()).then_some(Command::CompleteTask { id: card });
        }
        QueryStatus::Backlog => todo_core::model::WorkflowStatus::Backlog,
        QueryStatus::Todo => todo_core::model::WorkflowStatus::Todo,
        QueryStatus::InProgress => todo_core::model::WorkflowStatus::InProgress,
    };
    let mut from = None;
    let mut others = Vec::new();
    for (_, id) in state.siblings(Scope::Tasks(list)) {
        if let EntityId::Task(id) = id
            && state
                .tasks
                .get(&id)
                .is_some_and(|t| QueryStatus::of(t) == target && is_actionable(state, t))
        {
            if id == card {
                from = Some(others.len());
            } else {
                others.push(EntityId::Task(id));
            }
        }
    }
    let (index, placement) = match slot {
        Placement::First => (
            0,
            others
                .first()
                .map_or(Placement::First, |id| Placement::Before(*id)),
        ),
        Placement::Last => (
            others.len(),
            others
                .last()
                .map_or(Placement::First, |id| Placement::After(*id)),
        ),
        Placement::Before(anchor) => (others.iter().position(|id| *id == anchor)?, slot),
        Placement::After(anchor) => (others.iter().position(|id| *id == anchor)? + 1, slot),
    };
    (from != Some(index)).then_some(Command::MoveTaskStatus {
        id: card,
        list,
        status,
        placement,
    })
}
