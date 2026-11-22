//! A destructive action confirms the current name and impact. Clients keep the question shown and
//! compare it with a fresh read when the owner confirms it.
use super::*;

pub fn one_line(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

pub fn collection_question(state: &State, id: CollectionId) -> Option<String> {
    let collection = state
        .collections
        .get(&id)
        .filter(|_| state.is_live(id.into()))?;
    let name = one_line(&collection.name);
    let Contents { lists, tasks } = collection_contents(state, id);
    let lists_said = if lists == 1 {
        "its list".into()
    } else {
        format!("its {lists} lists")
    };
    let whose = if lists == 1 { "its" } else { "their" };
    let impact = match (lists, tasks) {
        (0, _) => String::new(),
        (_, 0) => format!(" with {lists_said}"),
        (_, 1) => format!(" with {lists_said} and {whose} task"),
        (_, n) => format!(" with {lists_said} and {whose} {n} tasks"),
    };
    let back = if lists == 0 { "it" } else { "them all" };
    Some(format!(
        "Delete the collection “{name}”{impact}? Undo brings {back} back."
    ))
}

pub fn tag_question(state: &State, id: TagId) -> Option<String> {
    let name = one_line(
        &state
            .tags
            .get(&id)
            .filter(|_| state.is_live(id.into()))?
            .name,
    );
    let n = state
        .tasks
        .values()
        .filter(|t| t.tags.contains(&id) && t.merged_into.is_none() && state.is_live(t.id.into()))
        .count();
    let impact = match n {
        0 => String::new(),
        1 => " It’s on 1 task. The task stays.".into(),
        n => format!(" It’s on {n} tasks. The tasks stay."),
    };
    Some(format!(
        "Delete the tag “{name}”?{impact} Undo restores the tag."
    ))
}
