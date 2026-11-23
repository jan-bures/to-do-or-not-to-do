//! The shared Merge into… picker and confirmation. The core still decides the atomic event batch.

use todo_core::ids::TaskId;
use todo_core::model::Task;
use todo_core::query::fold;
use todo_core::{Command, State};

use super::organize::{actionable_task, confirm::one_line};

pub const EMPTY: &str = "No other open tasks.";
pub const NO_MATCHES: &str = "No open tasks match.";
pub const CHANGED: &str = "The tasks changed while the picker was open. Check them and try again.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: TaskId,
    pub title: String,
    pub location: String,
}

fn candidate(state: &State, task: &Task) -> Candidate {
    let location = state
        .lists
        .get(&task.list)
        .map_or_else(String::new, |list| {
            let name = one_line(&list.name);
            list.collection
                .and_then(|id| state.collections.get(&id))
                .map_or_else(
                    || name.clone(),
                    |col| format!("{} / {name}", one_line(&col.name)),
                )
        });
    Candidate {
        id: task.id,
        title: one_line(&task.title),
        location,
    }
}

/// Search by title words (case and accents folded), across live lists. Only open tasks can be
/// destinations. Equal titles sort by location, then ID, so two clients see the same order.
pub fn candidates(state: &State, source: TaskId, filter: &str) -> Option<Vec<Candidate>> {
    actionable_task(state, source)?;
    let filter = fold(filter);
    let words: Vec<_> = filter.split_whitespace().collect();
    let mut matches: Vec<_> = state
        .tasks
        .values()
        .filter_map(|task| {
            if task.id == source
                || task.is_done()
                || task.merged_into.is_some()
                || !state.is_live(task.id.into())
            {
                return None;
            }
            let title = fold(&task.title);
            if !words.iter().all(|word| title.contains(word)) {
                return None;
            }
            let choice = candidate(state, task);
            Some(((title, fold(&choice.location), choice.id), choice))
        })
        .collect();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    Some(
        matches
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub question: String,
    pub toast: String,
}

pub fn preview(state: &State, source: TaskId, target: TaskId) -> Option<Preview> {
    if source == target {
        return None;
    }
    let source = candidate(state, actionable_task(state, source)?);
    let dest = actionable_task(state, target)?;
    if dest.is_done() {
        return None;
    }
    let dest = candidate(state, dest);
    let question = format!(
        "Merge “{}” ({}) into “{}” ({})? The destination keeps its own title and schedule and gains the source’s notes, steps, tags, links, attachments and reminders. Undo brings back the source.",
        source.title, source.location, dest.title, dest.location,
    );
    let toast = format!("Merged “{}” into “{}”", source.title, dest.title);
    Some(Preview { question, toast })
}

/// Build only what the owner saw. Recheck locally at submission; an offline client cannot check
/// for unseen changes on the server. The domain also validates the event batch (e.g. notes length).
pub fn command(state: &State, source: TaskId, target: TaskId, shown: &str) -> Option<Command> {
    (preview(state, source, target)?.question == shown)
        .then_some(Command::MergeTasks { source, target })
}
