//! Labels for task metadata shared by the native and web clients.

use todo_core::{
    State,
    model::{Tag, Task},
    query::fold,
};

use crate::intent::organize::confirm::one_line;

/// A list chip is either the list name or its collection followed by its name.
pub fn list_chip(name: &str, collection: Option<&str>) -> String {
    let name = one_line(name);
    collection.map_or_else(
        || name.clone(),
        |group| format!("{} · {name}", one_line(group)),
    )
}

/// A tag's name as compact, single-line metadata.
pub fn tag(name: &str) -> String {
    format!("#{}", one_line(name))
}

/// The task's visible tags in display order, retaining their IDs and colors.
pub fn tags<'a>(state: &'a State, task: &Task) -> Vec<&'a Tag> {
    let mut tags: Vec<_> = task
        .tags
        .iter()
        .filter_map(|id| state.tags.get(id))
        .filter(|tag| tag.deleted.is_none())
        .collect();
    tags.sort_by_cached_key(|tag| (fold(&tag.name), tag.id));
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_keep_identity_without_decorative_markers() {
        assert_eq!(list_chip("Planning", Some("Work")), "Work · Planning");
        assert_eq!(list_chip("Planning", None), "Planning");
        assert_eq!(tag("quarterly"), "#quarterly");
    }
}
