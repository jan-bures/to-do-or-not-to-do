//! Organizing: collections, moving lists between them, colours, tags and links. Pure, like the rest of
//! [`intent`](super): clients pass the state and a source of fresh IDs, and submit what comes back.
//!
//! A builder returns `None` when there is nothing to do (the change is already in place, or what it is
//! about has gone), so a client never submits a command the domain would only refuse or record as a
//! no-op.

use todo_core::State;
use todo_core::command::{Command, Placement};
use todo_core::decide::MAX_NAME;
use todo_core::ids::{CollectionId, EntityId, LinkId, ListId, TagId, TaskId};
use todo_core::model::{Collection, LinkKind, List, Tag, Task};
use todo_core::query::fold;
use todo_design::hues::{self, NamedHue};
use uuid::Uuid;

use super::delete;

pub mod confirm;

/// A list that is not deleted or hidden under a deleted collection.
#[must_use]
pub fn list_live(state: &State, id: ListId) -> bool {
    state.lists.get(&id).is_some_and(|list| {
        list.deleted.is_none()
            && list.collection.is_none_or(|id| {
                state
                    .collections
                    .get(&id)
                    .is_some_and(|c| c.deleted.is_none())
            })
    })
}

/// A task present in the client's active data: not deleted or merged, and in a live list.
/// Takes the borrowed task so an eligibility check never clones it or its ancestors.
#[must_use]
pub fn is_actionable(state: &State, task: &Task) -> bool {
    task.deleted.is_none() && task.merged_into.is_none() && list_live(state, task.list)
}

/// A fresh collection at the end of the sidebar, with a swatch picked from its ID (like a new list).
pub fn create_collection(id: CollectionId, name: &str) -> Command {
    let swatch = hues::swatch_for_seed(id.as_uuid().as_bytes());
    Command::CreateCollection {
        id,
        name: name.trim().to_string(),
        color: Some(swatch.key.to_owned()),
        placement: Placement::Last,
    }
}

/// Deletes a live collection. Its lists and their tasks go with it: they are hidden under the deleted
/// collection, not deleted themselves, so undoing (or restoring) the collection brings all of them back.
pub fn delete_collection(state: &State, id: CollectionId) -> Option<Command> {
    state.is_live(id.into()).then(|| delete(id))
}

/// What deleting a collection takes with it: its live lists and their live tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub lists: usize,
    pub tasks: usize,
}

/// The live lists of a collection and their live tasks (open and done), as its delete confirmation
/// counts them.
pub fn collection_contents(state: &State, id: CollectionId) -> Contents {
    let lists: Vec<ListId> = state
        .lists
        .values()
        .filter(|l| l.collection == Some(id) && l.deleted.is_none())
        .map(|l| l.id)
        .collect();
    let tasks = state
        .tasks
        .values()
        .filter(|t| lists.contains(&t.list) && t.deleted.is_none() && t.merged_into.is_none())
        .count();
    Contents {
        lists: lists.len(),
        tasks,
    }
}

/// The live collections, in sidebar order.
pub fn collections(state: &State) -> Vec<CollectionId> {
    let mut all: Vec<_> = state
        .collections
        .values()
        .filter(|c| state.is_live(c.id.into()))
        .collect();
    all.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    all.iter().map(|c| c.id).collect()
}

/// Moves a live list into another live collection (`Some`) or out of any (`None`), last among its new
/// siblings; `None` when it is already there or either is gone.
pub fn move_list(state: &State, id: ListId, collection: Option<CollectionId>) -> Option<Command> {
    let list = state.lists.get(&id)?;
    let live = state.is_live(id.into()) && collection.is_none_or(|c| state.is_live(c.into()));
    (live && list.collection != collection).then_some(Command::MoveList {
        id,
        collection,
        placement: Placement::Last,
    })
}

/// Whether a list has somewhere to move: out of its collection, or into a live collection it isn't in.
pub fn can_move_list(state: &State, id: ListId) -> bool {
    let Some(list) = state.lists.get(&id).filter(|_| state.is_live(id.into())) else {
        return false;
    };
    list.collection.is_some()
        || state
            .collections
            .keys()
            .any(|id| state.is_live((*id).into()))
}

/// A task that can be moved or deleted: it is live and is not a merged-away task.
pub fn actionable_task(state: &State, id: TaskId) -> Option<&Task> {
    state
        .tasks
        .get(&id)
        .filter(|task| is_actionable(state, task))
}

/// Moves a task to the first position in another list, provided it is still in the shown source.
/// Keep the source in the command too: the server may have moved it before the replica catches up.
pub fn move_task(state: &State, id: TaskId, from: ListId, to: ListId) -> Option<Command> {
    let task = actionable_task(state, id)?;
    (task.list == from && to != from && list_live(state, to)).then_some(Command::MoveTask {
        id,
        list: to,
        from_list: Some(from),
        placement: Placement::First,
    })
}

/// One section of the task's "Move to…" picker, in sidebar order.
#[derive(Debug)]
pub struct TaskMoveGroup<'a> {
    pub collection: Option<&'a Collection>,
    pub heading: Option<String>,
    pub lists: Vec<TaskMoveDestination<'a>>,
}

#[derive(Debug)]
pub struct TaskMoveDestination<'a> {
    pub list: &'a List,
    pub current: bool,
}

/// Every typed word must match the list or its collection, ignoring case and accents.
/// Empty groups are skipped. `None` means the task itself is no longer available.
pub fn task_move_groups<'a>(
    state: &'a State,
    id: TaskId,
    filter: &str,
) -> Option<Vec<TaskMoveGroup<'a>>> {
    let task = actionable_task(state, id)?;
    Some(task_destination_groups(state, filter, |id| id == task.list))
}

/// Shared list picker, with a caller-provided predicate marking unavailable (current) lists.
pub fn task_destination_groups<'a>(
    state: &'a State,
    filter: &str,
    current: impl Fn(ListId) -> bool,
) -> Vec<TaskMoveGroup<'a>> {
    let groups = super::list_groups(state);
    let has_collections = groups.iter().any(|(collection, _)| collection.is_some());
    let filter = fold(filter);
    let words: Vec<_> = filter.split_whitespace().collect();
    groups
        .into_iter()
        .filter_map(|(collection, ids)| {
            let collection = collection.and_then(|id| state.collections.get(&id));
            let collection_name = collection.map_or("", |c| c.name.as_str());
            let lists: Vec<_> = ids
                .into_iter()
                .filter_map(|id| state.lists.get(&id))
                .filter(|list| {
                    if words.is_empty() {
                        return true;
                    }
                    let name = fold(&format!("{} {collection_name}", list.name));
                    words.iter().all(|word| name.contains(word))
                })
                .map(|list| TaskMoveDestination {
                    list,
                    current: current(list.id),
                })
                .collect();
            (!lists.is_empty()).then(|| TaskMoveGroup {
                collection,
                heading: collection
                    .map(|c| confirm::one_line(&c.name))
                    .or_else(|| has_collections.then(|| "Other lists".into())),
                lists,
            })
        })
        .collect()
}

/// The swatch with this key (`"lagoon"`), if the design has one.
pub fn swatch(key: &str) -> Option<NamedHue> {
    hues::SWATCHES
        .iter()
        .copied()
        .find(|s| s.key.eq_ignore_ascii_case(key.trim()))
}

/// A display name for a swatch's stable key, for tooltips and accessible labels.
pub fn swatch_name(key: &str) -> String {
    let mut chars = key.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub fn colour_heading(id: EntityId) -> Option<&'static str> {
    Some(match id {
        EntityId::Collection(_) => "Collection colour",
        EntityId::List(_) => "List colour",
        EntityId::Tag(_) => "Tag colour",
        _ => return None,
    })
}

pub fn delete_recheck_text(kind: &str) -> String {
    format!("The {kind} changed while the question was open. Check it, then confirm again.")
}

/// Recolours a live collection, list or tag with one of the design's swatches. `None` when the key isn't a
/// swatch, the entity has no colour (or isn't live), or it already has exactly that colour.
pub fn set_color(state: &State, id: EntityId, key: &str) -> Option<Command> {
    let key = swatch(key)?.key;
    if !state.is_live(id) {
        return None;
    }
    let same = |c: &Option<String>| c.as_deref() == Some(key);
    let color = Some(Some(key.to_owned()));
    match id {
        EntityId::Collection(id) => {
            (!same(&state.collections.get(&id)?.color)).then_some(Command::UpdateCollection {
                id,
                name: None,
                color,
            })
        }
        EntityId::List(id) => {
            (!same(&state.lists.get(&id)?.color)).then_some(Command::UpdateList {
                id,
                name: None,
                color,
                features: None,
            })
        }
        EntityId::Tag(id) => (!same(&state.tags.get(&id)?.color)).then_some(Command::UpdateTag {
            id,
            name: None,
            color,
        }),
        _ => None,
    }
}

/// Why a typed link can't be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("nothing was typed")]
    Empty,
    #[error("only web addresses (http or https) can be added")]
    NotWeb,
    #[error("the task is gone")]
    TaskGone,
}

/// The web address a typed link stands for. What's typed is trimmed; `https://` goes in front only when
/// it doesn't start with a scheme of its own (`example.com/page`, `docs.example.com:8080/x`), and a scheme
/// is written in lower case. The result must parse as a URL (`url`, WHATWG) with an `http` or `https`
/// scheme and a host, and an address typed without a scheme must name a dotted host (`example.com`, not
/// `notes`). Then the core's own check (`todo_core::decide::validate_link`) must pass too, so what this
/// accepts the domain accepts. The address is kept as typed, not as the parser rewrites it.
pub fn web_address(text: &str) -> Result<String, LinkError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(LinkError::Empty);
    }
    let address = match scheme_of(text) {
        Some(scheme) => {
            let lower = scheme.to_ascii_lowercase();
            // `//` and then the host, as written: the parser would read `https:example.com` or
            // `https:///path` as some other address than the one kept.
            let rest = &text[scheme.len() + 1..];
            let authority = rest.strip_prefix("//").unwrap_or("/");
            if !matches!(lower.as_str(), "http" | "https") || authority.starts_with('/') {
                return Err(LinkError::NotWeb);
            }
            format!("{lower}{}", &text[scheme.len()..])
        }
        None => format!("https://{text}"),
    };
    let parsed = url::Url::parse(&address).map_err(|_| LinkError::NotWeb)?;
    let host = parsed.host_str().unwrap_or_default();
    let web = matches!(parsed.scheme(), "http" | "https") && !host.is_empty();
    let dotted = || {
        let name = host.trim_start_matches('[');
        name.contains('.') && !name.starts_with('.') && !name.ends_with('.')
    };
    if !web || (scheme_of(text).is_none() && !dotted()) {
        return Err(LinkError::NotWeb);
    }
    let kind = LinkKind::Web {
        url: address.clone(),
        title: None,
    };
    todo_core::decide::validate_link(&kind).map_err(|_| LinkError::NotWeb)?;
    Ok(address)
}

/// The scheme `text` starts with (`https` in `https://…`, `mailto` in `mailto:…`), if it starts with
/// one: letters, digits, `+`, `-` and `.` before the first `:`, beginning with a letter. A host followed
/// by a port (`example.com:8080`) isn't a scheme: a port starts with a digit.
fn scheme_of(text: &str) -> Option<&str> {
    let (scheme, rest) = text.split_once(':')?;
    let valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (valid && !rest.starts_with(|c: char| c.is_ascii_digit())).then_some(scheme)
}

/// Adds a web link to a live task, from what was typed (see [`web_address`]).
pub fn add_link(
    state: &State,
    task: TaskId,
    text: &str,
    new_id: impl FnOnce() -> Uuid,
) -> Result<Command, LinkError> {
    let url = web_address(text)?;
    if !state.is_live(task.into()) {
        return Err(LinkError::TaskGone);
    }
    Ok(Command::AddLink {
        id: LinkId::from_uuid(new_id()),
        task,
        kind: LinkKind::Web { url, title: None },
    })
}

/// Removes a live link (Undo brings it back); `None` when it is already gone.
pub fn delete_link(state: &State, id: LinkId) -> Option<Command> {
    state.is_live(id.into()).then(|| delete(id))
}

/// Why a tag name can't be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    #[error("a tag needs a name")]
    Empty,
    #[error("a tag's name can be at most {} characters", MAX_NAME)]
    TooLong,
    #[error("there is already a tag named “{0}”")]
    Taken(String),
    #[error("the task is gone")]
    TaskGone,
}

impl TagError {
    pub fn create_message(&self) -> String {
        format!("Couldn't create that tag: {self}.")
    }

    pub fn rename_message(&self) -> String {
        format!("Couldn't rename that tag: {self}.")
    }
}

/// A tag's name as the domain stores it (trimmed), or why it can't be one: empty, or longer than the
/// domain's limit (`todo_core::decide::MAX_NAME` characters).
fn tag_name(name: &str) -> Result<&str, TagError> {
    let name = name.trim();
    if name.is_empty() {
        Err(TagError::Empty)
    } else if name.chars().count() > MAX_NAME {
        Err(TagError::TooLong)
    } else {
        Ok(name)
    }
}

/// Every live tag, by name (ignoring case and accents).
pub fn tags(state: &State) -> Vec<&Tag> {
    let mut tags: Vec<&Tag> = state
        .tags
        .values()
        .filter(|t| state.is_live(t.id.into()))
        .collect();
    tags.sort_by_cached_key(|t| (fold(&t.name), t.id));
    tags
}

/// The live tag with this name, ignoring case as the domain does when it keeps names unique.
pub fn tag_named(state: &State, name: &str) -> Option<TagId> {
    let lower = name.trim().to_lowercase();
    state
        .tags
        .values()
        .find(|t| t.deleted.is_none() && t.name.to_lowercase() == lower)
        .map(|t| t.id)
}

/// A new tag, with a swatch picked from its ID. Refused when the name is empty, longer than the domain
/// allows, or another live tag has it (ignoring case, as the domain does: `str::to_lowercase`, not the
/// search fold, so `é` written as one character or as `e` and an accent are two names).
pub fn create_tag(state: &State, id: TagId, name: &str) -> Result<Command, TagError> {
    let name = tag_name(name)?;
    if let Some(other) = tag_named(state, name).and_then(|t| state.tags.get(&t)) {
        return Err(TagError::Taken(other.name.clone()));
    }
    let swatch = hues::swatch_for_seed(id.as_uuid().as_bytes());
    Ok(Command::CreateTag {
        id,
        name: name.to_string(),
        color: Some(swatch.key.to_owned()),
    })
}

/// Renames a live tag. `Ok(None)` when the name is unchanged or the tag is gone; refused when it is empty,
/// too long, or another live tag has it.
pub fn rename_tag(state: &State, id: TagId, name: &str) -> Result<Option<Command>, TagError> {
    let Some(tag) = state.tags.get(&id).filter(|_| state.is_live(id.into())) else {
        return Ok(None);
    };
    let name = tag_name(name)?;
    if name == tag.name {
        return Ok(None);
    }
    if let Some(other) = tag_named(state, name)
        .filter(|&t| t != id)
        .and_then(|t| state.tags.get(&t))
    {
        return Err(TagError::Taken(other.name.clone()));
    }
    Ok(Some(Command::UpdateTag {
        id,
        name: Some(name.to_string()),
        color: None,
    }))
}

/// Tags a live task with a live tag, or takes the tag off; `None` when either is gone.
pub fn toggle_tag(state: &State, task: TaskId, tag: TagId) -> Option<Command> {
    let t = state
        .tasks
        .get(&task)
        .filter(|_| state.is_live(task.into()))?;
    if t.tags.contains(&tag) {
        Some(Command::RemoveTag { task, tag })
    } else {
        state
            .is_live(tag.into())
            .then_some(Command::AddTag { task, tag })
    }
}

/// Tags a live task by name: with the live tag of that name, or with a new one created first. The new
/// tag's ID, if one is created, comes back with the commands; empty when the task already has the tag.
/// Refused, creating nothing, when the task is gone, so no tag is made that nothing carries.
pub fn tag_task_named(
    state: &State,
    task: TaskId,
    name: &str,
    new_id: impl FnOnce() -> Uuid,
) -> Result<(TagId, Vec<Command>), TagError> {
    if !state.is_live(task.into()) {
        return Err(TagError::TaskGone);
    }
    if let Some(tag) = tag_named(state, name) {
        let on = state
            .tasks
            .get(&task)
            .is_some_and(|t| t.tags.contains(&tag));
        let cmds = if on {
            Vec::new()
        } else {
            toggle_tag(state, task, tag).into_iter().collect()
        };
        return Ok((tag, cmds));
    }
    let tag = TagId::from_uuid(new_id());
    let create = create_tag(state, tag, name)?;
    Ok((tag, vec![create, Command::AddTag { task, tag }]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{TaskRoom, add_task, create_list, quick_add::Parsed};
    use todo_core::actor::Actor;
    use todo_core::{apply, decide};

    struct World {
        state: State,
        next: u64,
    }

    impl World {
        fn new() -> Self {
            Self {
                state: State::new(),
                next: 1,
            }
        }

        fn id(&mut self) -> Uuid {
            self.next += 1;
            todo_core::ids::uuid_v7(1_790_000_000_000 + self.next, [self.next as u8; 10])
        }

        /// Runs a command the domain must accept.
        fn run(&mut self, cmd: Command) {
            let actor = Actor::owner("test").expect("actor");
            // Every command at its own second: a repeated command is a new event.
            self.next += 1;
            let (m, sec) = (self.next / 60 % 60, self.next % 60);
            let now = format!("2026-11-21T10:{m:02}:{sec:02}Z")
                .parse()
                .expect("ts");
            let events = decide(&self.state, &actor, &cmd, now).expect("valid");
            for e in &events {
                apply(&mut self.state, e).expect("applies");
            }
        }

        fn collection(&mut self, name: &str) -> CollectionId {
            let id = CollectionId::from_uuid(self.id());
            self.run(create_collection(id, name));
            id
        }

        fn list(&mut self, collection: Option<CollectionId>, name: &str) -> ListId {
            let id = ListId::from_uuid(self.id());
            self.run(create_list(id, collection, name));
            id
        }

        fn task(&mut self, list: ListId, title: &str) -> TaskId {
            let state = self.state.clone();
            let parsed = Parsed {
                title: title.into(),
                ..Parsed::default()
            };
            let today = "2026-11-21".parse().expect("date");
            let (id, cmds) = add_task(&state, TaskRoom::List(list), &parsed, today, || self.id());
            for c in cmds {
                self.run(c);
            }
            id.expect("task")
        }
    }

    #[test]
    fn task_destinations_follow_sidebar_order_and_match_folded_words_across_names() {
        let mut w = World::new();
        let first = w.collection("Dům");
        let current = w.list(Some(first), "Opravy");
        let other = w.list(Some(first), "Zahrada");
        let empty = w.collection("Empty");
        let dead = w.list(Some(empty), "Zahrada");
        let loose = w.list(None, "Inbox");
        let task = w.task(current, "Fix the tap");
        w.run(delete(dead));

        let groups = task_move_groups(&w.state, task, "").unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].heading.as_deref(), Some("Dům"));
        assert_eq!(groups[0].collection.map(|c| c.id), Some(first));
        assert_eq!(
            groups[0]
                .lists
                .iter()
                .map(|d| (d.list.id, d.current))
                .collect::<Vec<_>>(),
            vec![(current, true), (other, false)]
        );
        assert_eq!(groups[1].heading.as_deref(), Some("Other lists"));
        assert_eq!(groups[1].lists[0].list.id, loose);

        let groups = task_move_groups(&w.state, task, "  ZÁH dum ").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].lists.len(), 1);
        assert_eq!(groups[0].lists[0].list.id, other);
        assert!(
            task_move_groups(&w.state, task, "empty")
                .unwrap()
                .is_empty()
        );
        w.run(delete(task));
        assert!(task_move_groups(&w.state, task, "").is_none());
    }

    #[test]
    fn merge_search_matches_open_live_titles_across_lists_in_stable_order() {
        use crate::intent::merge;
        let mut w = World::new();
        let group = w.collection("Hobby");
        let a = w.list(None, "Errands");
        let b = w.list(Some(group), "Workshop");
        let source = w.task(a, "Seřídit kolo");
        let second = w.task(b, "Seřídit kolo");
        let first = w.task(a, "Seřídit kolo");
        let earlier = w.task(b, "Kolo seřídit");
        let done = w.task(a, "Seřídit kolo znovu");
        let hidden_group = w.collection("Old");
        let hidden = w.list(Some(hidden_group), "Old list");
        w.task(hidden, "Seřídit kolo dnes");
        w.run(delete(hidden_group));
        w.run(Command::CompleteTask { id: done });
        assert_eq!(
            merge::candidates(&w.state, source, "  KOLO serid ")
                .unwrap()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [earlier, first, second]
        );
        assert!(
            merge::candidates(&w.state, source, "workshop")
                .unwrap()
                .is_empty()
        );
        assert!(merge::preview(&w.state, source, source).is_none());
        assert!(merge::preview(&w.state, source, done).is_none());
    }

    #[test]
    fn merge_rechecks_the_confirmation_and_cannot_reuse_a_tombstoned_source() {
        use crate::intent::merge;
        let mut w = World::new();
        let list = w.list(None, "Errands");
        let source = w.task(list, "Buy stamps");
        let target = w.task(list, "Go to the post office");
        let shown = merge::preview(&w.state, source, target).unwrap();
        assert!(
            shown
                .question
                .contains("notes, steps, tags, links, attachments and reminders")
        );
        assert!(
            shown
                .question
                .contains("“Buy stamps” (Errands) into “Go to the post office” (Errands)")
        );
        w.state.tasks.get_mut(&target).unwrap().title = "Post the parcel".into();
        assert!(merge::command(&w.state, source, target, &shown.question).is_none());
        let shown = merge::preview(&w.state, source, target).unwrap();
        let cmd = merge::command(&w.state, source, target, &shown.question).unwrap();
        w.run(cmd);
        assert_eq!(w.state.tasks[&source].merged_into, Some(target));
        assert!(merge::candidates(&w.state, source, "").is_none());
        assert!(merge::command(&w.state, source, target, &shown.question).is_none());
    }

    #[test]
    fn a_picker_cannot_move_from_a_stale_source_or_act_on_a_restored_merged_task() {
        let mut w = World::new();
        let first = w.list(None, "First");
        let second = w.list(None, "Second");
        let third = w.list(None, "Third");
        let task = w.task(first, "Proof");
        let target = w.task(second, "Draft");
        w.run(move_task(&w.state, task, first, second).unwrap());
        assert!(move_task(&w.state, task, first, third).is_none());
        assert!(move_task(&w.state, task, second, third).is_some());
        w.run(Command::MergeTasks {
            source: task,
            target,
        });
        w.run(Command::Restore { id: task.into() });
        assert!(
            w.state.is_live(task.into()),
            "restore clears only the tombstone"
        );
        assert!(task_move_groups(&w.state, task, "").is_none());
        assert!(move_task(&w.state, task, second, third).is_none());
        assert!(super::super::delete_task(&w.state, task).is_none());
    }

    #[test]
    fn confirmation_questions_count_only_live_contents_and_word_names_on_one_line() {
        let mut w = World::new();
        let c = w.collection("Front\nyard");
        let collection = |s: &State| confirm::collection_question(s, c);
        assert_eq!(
            collection(&w.state).as_deref(),
            Some("Delete the collection “Front yard”? Undo brings it back.")
        );
        let list = w.list(Some(c), "Beds");
        let task = w.task(list, "Plant bulbs");
        let tag = TagId::from_uuid(w.id());
        w.run(create_tag(&w.state, tag, "spring").unwrap());
        w.run(toggle_tag(&w.state, task, tag).unwrap());
        assert_eq!(
            collection(&w.state).as_deref(),
            Some(
                "Delete the collection “Front yard” with its list and its task? Undo brings them all back."
            )
        );
        assert_eq!(
            confirm::tag_question(&w.state, tag).as_deref(),
            Some("Delete the tag “spring”? It’s on 1 task. The task stays. Undo restores the tag.")
        );
        w.run(delete(c));
        assert!(collection(&w.state).is_none());
        assert_eq!(
            confirm::tag_question(&w.state, tag).as_deref(),
            Some("Delete the tag “spring”? Undo restores the tag.")
        );
        w.run(delete(tag));
        assert!(confirm::tag_question(&w.state, tag).is_none());
    }

    #[test]
    fn a_new_collection_is_last_with_a_swatch_and_deleting_it_hides_its_lists() {
        let mut w = World::new();
        let work = w.collection(" Work ");
        let home = w.collection("Home");
        assert_eq!(collections(&w.state), [work, home]);
        assert_eq!(w.state.collections[&work].name, "Work");
        let color = w.state.collections[&work].color.clone().expect("a colour");
        assert!(swatch(&color).is_some(), "{color}");

        let office = w.list(Some(work), "Office");
        let travel = w.list(Some(work), "Travel");
        let loose = w.list(None, "Loose");
        w.task(office, "Print");
        w.task(travel, "Pack");
        let gone = w.task(travel, "Book");
        w.run(delete(gone));
        w.task(loose, "Elsewhere");
        assert_eq!(
            collection_contents(&w.state, work),
            Contents { lists: 2, tasks: 2 }
        );
        assert_eq!(collection_contents(&w.state, home), Contents::default());

        w.run(delete_collection(&w.state, work).expect("delete"));
        // The lists themselves aren't deleted, only hidden with their collection.
        assert!(w.state.lists[&office].deleted.is_none());
        assert!(!w.state.is_live(office.into()));
        assert_eq!(collections(&w.state), [home]);
        assert!(delete_collection(&w.state, work).is_none(), "already gone");
        // Restoring it brings everything back.
        w.run(Command::Restore { id: work.into() });
        assert!(w.state.is_live(office.into()) && w.state.is_live(travel.into()));
    }

    #[test]
    fn a_list_moves_into_another_collection_or_out_of_any() {
        let mut w = World::new();
        let work = w.collection("Work");
        let home = w.collection("Home");
        let garden = w.list(Some(home), "Garden");
        let office = w.list(Some(work), "Office");
        let loose = w.list(None, "Loose");
        assert!(
            move_list(&w.state, office, Some(work)).is_none(),
            "already there"
        );
        w.run(move_list(&w.state, office, Some(home)).expect("move"));
        assert_eq!(w.state.lists[&office].collection, Some(home));
        assert!(
            w.state.lists[&garden].position < w.state.lists[&office].position,
            "last among its new siblings"
        );
        w.run(move_list(&w.state, office, None).expect("out"));
        assert_eq!(w.state.lists[&office].collection, None);
        assert!(w.state.lists[&loose].position < w.state.lists[&office].position);
        assert!(move_list(&w.state, office, None).is_none());
        // Never into a deleted collection; a deleted list isn't moved.
        w.run(delete(work));
        assert!(move_list(&w.state, office, Some(work)).is_none());
        w.run(delete(loose));
        assert!(move_list(&w.state, loose, Some(home)).is_none());
        assert!(!can_move_list(&w.state, loose));
        assert!(can_move_list(&w.state, office));
        assert!(can_move_list(&w.state, garden), "out of its collection");
        // A top-level list with no collection to go to has nowhere to move.
        let mut alone = World::new();
        let only = alone.list(None, "Only");
        assert!(!can_move_list(&alone.state, only));
    }

    #[test]
    fn colours_are_the_designs_swatches_and_unchanged_ones_send_nothing() {
        let mut w = World::new();
        let work = w.collection("Work");
        let list = w.list(Some(work), "Office");
        let tag = TagId::from_uuid(w.id());
        w.run(create_tag(&w.state, tag, "docs").expect("tag"));
        for id in [EntityId::from(work), list.into(), tag.into()] {
            w.run(set_color(&w.state, id, "Lagoon").expect("recolour"));
            assert!(set_color(&w.state, id, "lagoon").is_none(), "unchanged");
            // Only the design's swatches, never a free-form colour.
            assert!(set_color(&w.state, id, "#ff0000").is_none());
            assert!(set_color(&w.state, id, "").is_none());
        }
        assert_eq!(w.state.collections[&work].color.as_deref(), Some("lagoon"));
        assert_eq!(w.state.lists[&list].color.as_deref(), Some("lagoon"));
        assert_eq!(w.state.tags[&tag].color.as_deref(), Some("lagoon"));
        // Other entities have no colour; a gone one isn't recoloured.
        let task = w.task(list, "Print");
        assert!(set_color(&w.state, task.into(), "rose").is_none());
        w.run(delete(work));
        assert!(set_color(&w.state, list.into(), "rose").is_none());
        for s in hues::SWATCHES {
            assert_eq!(swatch(s.key), Some(s));
        }
    }

    #[test]
    fn typed_links_become_web_addresses_the_domain_accepts() {
        assert_eq!(
            web_address(" https://example.com/a?b=1 ").as_deref(),
            Ok("https://example.com/a?b=1")
        );
        assert_eq!(
            web_address("HTTP://Example.com/Path").as_deref(),
            Ok("http://Example.com/Path")
        );
        assert_eq!(
            web_address("example.com/page").as_deref(),
            Ok("https://example.com/page")
        );
        assert_eq!(
            web_address("docs.example.com:8080/x").as_deref(),
            Ok("https://docs.example.com:8080/x")
        );
        // A scheme anywhere but at the start is part of the address.
        assert_eq!(
            web_address("example.com/?next=https://example.com").as_deref(),
            Ok("https://example.com/?next=https://example.com")
        );
        assert_eq!(
            web_address("HTTPS://Example.com/Path").as_deref(),
            Ok("https://Example.com/Path")
        );
        assert_eq!(
            web_address("https://localhost:8080/x").as_deref(),
            Ok("https://localhost:8080/x")
        );
        assert_eq!(web_address("   "), Err(LinkError::Empty));
        for bad in [
            "javascript:alert(1)",
            "mailto:you@example.com",
            "file:///C:/Windows",
            "ftp://example.com",
            "just words",
            "https://exa mple.com",
            "localhost",
            "notes",
            ".com",
            "example.",
            // A scheme with nothing after it would save a card that opens nothing.
            "https://",
            "http://",
            "https:///path",
            "https:example.com",
            "HTTPS://",
        ] {
            assert_eq!(web_address(bad), Err(LinkError::NotWeb), "{bad}");
        }
        // Too long for the domain.
        let long = format!("https://example.com/{}", "a".repeat(4096));
        assert_eq!(web_address(&long), Err(LinkError::NotWeb));
    }

    #[test]
    fn a_link_is_added_to_a_live_task_and_removed_once() {
        let mut w = World::new();
        let list = w.list(None, "Errands");
        let task = w.task(list, "Read");
        let state = w.state.clone();
        let cmd = add_link(&state, task, "example.com/article", || w.id()).expect("link");
        let Command::AddLink { id, .. } = cmd else {
            panic!("{cmd:?}");
        };
        w.run(cmd);
        assert!(matches!(
            &w.state.links[&id].kind,
            LinkKind::Web { url, title: None } if url == "https://example.com/article"
        ));
        w.run(delete_link(&w.state, id).expect("remove"));
        assert!(delete_link(&w.state, id).is_none());
        w.run(Command::Restore { id: id.into() });
        assert!(w.state.is_live(id.into()));
        assert_eq!(
            add_link(&w.state, task, "not a link", Uuid::nil),
            Err(LinkError::NotWeb)
        );
        w.run(delete(task));
        assert_eq!(
            add_link(&w.state, task, "example.com", Uuid::nil),
            Err(LinkError::TaskGone)
        );
    }

    #[test]
    fn tags_are_created_once_by_name_renamed_and_toggled_on_a_task() {
        let mut w = World::new();
        let list = w.list(None, "Errands");
        let task = w.task(list, "Taxes");
        let fresh = TagId::from_uuid(w.id());
        assert_eq!(create_tag(&w.state, fresh, " "), Err(TagError::Empty));
        // The domain's limit on names, counted in characters.
        let long = "ř".repeat(MAX_NAME + 1);
        assert_eq!(create_tag(&w.state, fresh, &long), Err(TagError::TooLong));
        assert!(create_tag(&w.state, fresh, &"ř".repeat(MAX_NAME)).is_ok());
        // By name: created first, then put on the task.
        let state = w.state.clone();
        let (docs, cmds) = tag_task_named(&state, task, " docs ", || w.id()).expect("tag");
        assert_eq!(cmds.len(), 2);
        for c in cmds {
            w.run(c);
        }
        assert_eq!(w.state.tags[&docs].name, "docs");
        assert!(
            w.state.tags[&docs]
                .color
                .as_deref()
                .and_then(swatch)
                .is_some()
        );
        assert!(w.state.tasks[&task].tags.contains(&docs));
        // The same name in another case finds it, and a tagged task needs nothing more.
        assert_eq!(tag_named(&w.state, "DOCS"), Some(docs));
        let (again, cmds) = tag_task_named(&w.state, task, "Docs", Uuid::nil).expect("found");
        assert_eq!((again, cmds.len()), (docs, 0));
        let fresh = TagId::from_uuid(w.id());
        assert_eq!(
            create_tag(&w.state, fresh, "DOCS"),
            Err(TagError::Taken("docs".into()))
        );
        // Toggling takes it off and puts it back.
        w.run(toggle_tag(&w.state, task, docs).expect("off"));
        assert!(!w.state.tasks[&task].tags.contains(&docs));
        let (_, cmds) = tag_task_named(&w.state, task, "docs", Uuid::nil).expect("found");
        assert_eq!(cmds, [Command::AddTag { task, tag: docs }]);
        w.run(toggle_tag(&w.state, task, docs).expect("on"));
        assert!(w.state.tasks[&task].tags.contains(&docs));

        // Renaming: unchanged sends nothing, a taken name is refused, case changes are allowed.
        let receipts = TagId::from_uuid(w.id());
        w.run(create_tag(&w.state, receipts, "receipts").expect("tag"));
        assert_eq!(rename_tag(&w.state, docs, " docs "), Ok(None));
        assert_eq!(rename_tag(&w.state, docs, ""), Err(TagError::Empty));
        assert_eq!(
            rename_tag(&w.state, docs, "Receipts"),
            Err(TagError::Taken("receipts".into()))
        );
        w.run(
            rename_tag(&w.state, docs, "Docs")
                .expect("ok")
                .expect("cmd"),
        );
        assert_eq!(w.state.tags[&docs].name, "Docs");
        assert_eq!(
            tags(&w.state).iter().map(|t| t.id).collect::<Vec<_>>(),
            [docs, receipts]
        );
        // A deleted tag can't be put on a task (taking it off still works) or renamed.
        w.run(delete(receipts));
        assert!(toggle_tag(&w.state, task, receipts).is_none());
        assert_eq!(rename_tag(&w.state, receipts, "x"), Ok(None));
        assert_eq!(tags(&w.state).len(), 1);
        // Its name is free again.
        let fresh = TagId::from_uuid(w.id());
        assert!(create_tag(&w.state, fresh, "receipts").is_ok());
        assert_eq!(
            rename_tag(&w.state, docs, &"x".repeat(MAX_NAME + 1)),
            Err(TagError::TooLong)
        );
        // A gone task is never tagged, and no tag is created for it.
        w.run(delete(task));
        assert!(toggle_tag(&w.state, task, docs).is_none());
        assert_eq!(
            tag_task_named(&w.state, task, "orphan", Uuid::nil),
            Err(TagError::TaskGone)
        );
        assert_eq!(tag_named(&w.state, "orphan"), None);
    }
}
