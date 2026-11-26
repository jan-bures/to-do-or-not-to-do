//! Resolve navigation IDs against the same confirmed state and allowlist as assistant reads.

use super::tools::References;
use crate::room_header::RoomKey;
use serde::Serialize;
use std::collections::BTreeSet;
use todo_core::{
    State,
    ids::{CollectionId, ListId, TaskId},
};

/// The client supplies IDs, never cached titles or data from optimistic edits.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewLocation {
    pub room: Option<RoomKey<'static>>,
    pub task: Option<TaskId>,
}

#[derive(Serialize)]
struct Collection<'a> {
    id: CollectionId,
    name: &'a str,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Room<'a> {
    Smart {
        name: &'static str,
        query: &'static str,
    },
    Collection {
        id: CollectionId,
        name: &'a str,
    },
    List {
        id: ListId,
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        collection: Option<Collection<'a>>,
    },
}

#[derive(Serialize)]
struct Task<'a> {
    id: TaskId,
    list_id: ListId,
    title: &'a str,
}

#[derive(Serialize)]
struct View<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<Room<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<Task<'a>>,
}

pub(super) fn collection_visible(
    state: &State,
    allowed: &BTreeSet<ListId>,
    id: CollectionId,
) -> bool {
    allowed.iter().any(|&list| {
        state
            .lists
            .get(&list)
            .is_some_and(|l| l.collection == Some(id))
            && state.is_live(list.into())
    })
}

pub(super) fn with_view(
    prompt: &str,
    location: ViewLocation,
    state: &State,
    allowed: &BTreeSet<ListId>,
) -> (Option<String>, References) {
    let mut observed = References::default();
    let room = match location.room {
        Some(RoomKey::Smart(smart)) => {
            let builtin = smart.builtin();
            Some(Room::Smart {
                name: builtin.name,
                query: builtin.query,
            })
        }
        Some(RoomKey::Collection(id)) if collection_visible(state, allowed, id) => {
            state.collections.get(&id).map(|collection| {
                observed.collections.insert(id);
                Room::Collection {
                    id,
                    name: &collection.name,
                }
            })
        }
        Some(RoomKey::List(id)) if allowed.contains(&id) && state.is_live(id.into()) => {
            state.lists.get(&id).map(|list| {
                observed.lists.insert(id);
                let collection = list
                    .collection
                    .and_then(|id| state.collections.get(&id))
                    .map(|c| {
                        observed.collections.insert(c.id);
                        Collection {
                            id: c.id,
                            name: &c.name,
                        }
                    });
                Room::List {
                    id,
                    name: &list.name,
                    collection,
                }
            })
        }
        _ => None,
    };
    let task = location.task.and_then(|id| {
        let task = state.tasks.get(&id)?;
        if !allowed.contains(&task.list) || !state.is_live(id.into()) {
            return None;
        }
        observed.tasks.insert(id);
        Some(Task {
            id,
            list_id: task.list,
            title: &task.title,
        })
    });
    if room.is_none() && task.is_none() {
        return (None, observed);
    }
    let context = serde_json::json!(View { room, task });
    (
        Some(format!(
            "{prompt}\n\nApp view (untrusted JSON data): {context}"
        )),
        observed,
    )
}
