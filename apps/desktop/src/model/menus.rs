//! What each popover menu offers, as data: the room's menu (the header's "more" button), a list's, a
//! collection's or a task's context menu, the account menu, the priority picker, the list picker that
//! moves a task, and the "couldn't sync" list. The view draws the entries and runs an item's
//! [`MenuAction`]; the keyboard walks the same items, so every menu behaves alike. Pure, so what a menu
//! offers in each room is unit-tested.

use gpui::SharedString;
use todo_client::CommandId;
use todo_client::outbox::FailedEntry;
use todo_core::State;
use todo_core::ids::{CollectionId, EntityId, ListId, TaskId};
use todo_core::model::Priority;
use todo_core::query::fold;
use todo_core::time::Date;

use super::{RoomKey, commands, labels, lists};
use crate::theme::{Glyph, Hue, Icon};

/// What choosing a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Rename in place, in the sidebar.
    RenameList(ListId),
    RenameCollection(CollectionId),
    /// Show or hide the room's Completed section.
    ToggleCompleted,
    /// Ask before deleting: the menu turns into the confirmation.
    AskDeleteList(ListId),
    /// Leave a confirmation without doing anything: the menu closes.
    Cancel,
    /// Delete the list and its tasks (Undo brings them back).
    DeleteList(ListId),
    /// One place up (`-1`) or down (`1`) among its siblings.
    Nudge(EntityId, i32),
    /// Fold or unfold a collection in the sidebar.
    ToggleCollection(CollectionId),
    NewListIn(CollectionId),
    /// Open the server's admin pages in the browser.
    OpenAdmin,
    /// Retry syncing now instead of waiting out the backoff.
    SyncNow,
    /// Ask before signing out: the item turns into the confirmation.
    AskSignOut,
    SignOut,
    SetPriority(TaskId, Priority),
    /// A task's context menu: complete or reopen it.
    ToggleDone(TaskId),
    ToggleMyDay(TaskId),
    ToggleImportant(TaskId),
    /// Turns the task's menu into the priority picker, in place.
    AskPriority(TaskId),
    /// Turns the task's menu into the list picker, in place.
    AskMove(TaskId),
    /// Moves the task to the list (nothing when it's there already).
    MoveTask(TaskId, ListId),
    /// Deletes the task, without asking: Undo brings it back.
    DeleteTask(TaskId),
    /// Takes one refused change off the "couldn't sync" list.
    DismissFailure(CommandId),
    DismissAllFailures,
}

impl MenuAction {
    /// Whether choosing it takes the keyboard somewhere of its own: an editor opens (renaming, a new
    /// list), the room it was opened from goes away (deleting the list), or the whole window changes
    /// (signing out). Any other item chosen from the keyboard gives the keyboard back to the menu's
    /// button.
    pub fn moves_focus(self) -> bool {
        matches!(
            self,
            Self::RenameList(_)
                | Self::RenameCollection(_)
                | Self::NewListIn(_)
                | Self::DeleteList(_)
                | Self::SignOut
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// The element ID, unique within its menu.
    pub key: SharedString,
    pub icon: Icon,
    /// A list's own glyph and hue, drawn instead of the icon (the list picker).
    pub glyph: Option<(Glyph, Hue)>,
    pub label: SharedString,
    /// Destructive: drawn in the danger colour.
    pub danger: bool,
    /// The current choice (a tick on the right).
    pub checked: bool,
    /// Quiet text on the right, such as the priority's marks.
    pub hint: Option<SharedString>,
    pub action: MenuAction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
    /// A quiet label over a value, such as the server the app is signed in to.
    Caption {
        label: &'static str,
        value: SharedString,
    },
    /// A confirmation: what will happen, in full (it wraps, never truncates), above two short buttons:
    /// the action, then Cancel.
    Confirm {
        message: SharedString,
        buttons: [MenuItem; 2],
    },
    /// A quiet section label: a collection's name in the list picker.
    Heading(SharedString),
    /// What has been typed to narrow the list picker (empty: a hint that typing filters).
    Filter(SharedString),
    /// A quiet line that wraps, such as "No list matches".
    Note(SharedString),
    /// A change the server refused: what it was and why, beside its Dismiss button.
    Failure {
        what: SharedString,
        why: SharedString,
        dismiss: MenuItem,
    },
}

impl MenuEntry {
    /// The items it offers, in the order the keyboard walks them.
    pub fn items(&self) -> &[MenuItem] {
        match self {
            Self::Item(item) => std::slice::from_ref(item),
            Self::Failure { dismiss, .. } => std::slice::from_ref(dismiss),
            Self::Confirm { buttons, .. } => buttons,
            Self::Separator
            | Self::Caption { .. }
            | Self::Heading(_)
            | Self::Filter(_)
            | Self::Note(_) => &[],
        }
    }
}

fn item(
    key: impl Into<SharedString>,
    icon: Icon,
    label: impl Into<SharedString>,
    action: MenuAction,
) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        key: key.into(),
        icon,
        glyph: None,
        label: label.into(),
        danger: false,
        checked: false,
        hint: None,
        action,
    })
}

fn danger(
    key: impl Into<SharedString>,
    icon: Icon,
    label: impl Into<SharedString>,
    action: MenuAction,
) -> MenuEntry {
    match item(key, icon, label, action) {
        MenuEntry::Item(i) => MenuEntry::Item(MenuItem { danger: true, ..i }),
        other => other,
    }
}

/// The items of `entries`, in order: what the keyboard walks.
pub fn items(entries: &[MenuEntry]) -> impl Iterator<Item = &MenuItem> {
    entries.iter().flat_map(MenuEntry::items)
}

/// The entries that scroll in a long menu: all but the list picker's filter line, which stays put above
/// them.
pub fn scrolling(entries: &[MenuEntry]) -> impl Iterator<Item = &MenuEntry> {
    entries
        .iter()
        .filter(|e| !matches!(e, MenuEntry::Filter(_)))
}

/// Which of the [`scrolling`] entries holds item `item` (as the keyboard counts them): the one to
/// scroll into view when the keyboard moves there.
pub fn child_of_item(entries: &[MenuEntry], item: usize) -> Option<usize> {
    let mut seen = 0;
    scrolling(entries).position(|e| {
        let n = e.items().len();
        let here = item >= seen && item < seen + n;
        seen += n;
        here
    })
}

/// Whether the menu is a confirmation (drawn wider, its buttons side by side).
pub fn is_confirmation(entries: &[MenuEntry]) -> bool {
    entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Confirm { .. }))
}

/// Whether the menu is drawn at the confirmation's width: a confirmation, or the "couldn't sync" list,
/// whose reasons wrap beside their buttons.
pub fn is_wide(entries: &[MenuEntry]) -> bool {
    entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Confirm { .. } | MenuEntry::Failure { .. }))
}

/// Where the keyboard starts in a menu opened from the keyboard: on the current choice (the priority
/// picker's level), on Cancel in a confirmation, so a second Enter never destroys anything, else on the
/// first item.
pub fn first_highlight(entries: &[MenuEntry]) -> Option<usize> {
    let items: Vec<&MenuItem> = items(entries).collect();
    if items.is_empty() {
        return None;
    }
    let at = (items.iter().position(|i| i.checked))
        .or_else(|| items.iter().position(|i| i.action == MenuAction::Cancel))
        .unwrap_or(0);
    Some(at)
}

/// The keyboard highlight after pressing Up (`-1`) or Down (`1`) in a menu of `count` items: it wraps
/// around, and the first press lands on the first (Down) or last (Up) item.
pub fn step_highlight(current: Option<usize>, by: i32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match current {
        None if by < 0 => count - 1,
        None => 0,
        Some(i) if by < 0 => (i + count - 1) % count,
        Some(i) => (i + 1) % count,
    })
}

/// Whether the room has a Completed section to show or hide: the Completed and Agent activity smart
/// lists already show finished tasks among their rows.
fn has_completed_section(room: RoomKey) -> bool {
    match room {
        RoomKey::Smart(s) => s.done_query().is_some(),
        RoomKey::List(_) | RoomKey::Collection(_) => true,
    }
}

fn completed_item(show_completed: bool) -> MenuEntry {
    if show_completed {
        item(
            "room-completed",
            Icon::EyeOff,
            "Hide completed",
            MenuAction::ToggleCompleted,
        )
    } else {
        item(
            "room-completed",
            Icon::Eye,
            "Show completed",
            MenuAction::ToggleCompleted,
        )
    }
}

/// A confirmation: `message` in full, then `action` (destructive) beside Cancel.
fn confirm(message: String, action: MenuEntry, cancel_key: &'static str) -> MenuEntry {
    let cancel = item(cancel_key, Icon::X, "Cancel", MenuAction::Cancel);
    match (action, cancel) {
        (MenuEntry::Item(action), MenuEntry::Item(cancel)) => MenuEntry::Confirm {
            message: message.into(),
            buttons: [action, cancel],
        },
        (other, _) => other,
    }
}

/// The confirmation that replaces a list's menu once Delete was chosen: it says, in full, which list and
/// how many tasks go with it.
fn delete_list_confirm(
    state: &State,
    id: ListId,
    keys: (&'static str, &'static str),
) -> Option<MenuEntry> {
    let list = state.lists.get(&id).filter(|_| state.is_live(id.into()))?;
    let name = labels::one_line(&list.name);
    let tasks = state
        .tasks
        .values()
        .filter(|t| t.list == id && t.deleted.is_none())
        .count();
    let what = match tasks {
        0 => format!("Delete the list “{name}”?"),
        1 => format!("Delete the list “{name}” and its task?"),
        n => format!("Delete the list “{name}” and its {n} tasks?"),
    };
    let back = if tasks == 0 { "it" } else { "them" };
    Some(confirm(
        format!("{what} Undo brings {back} back."),
        danger(
            keys.0,
            Icon::TrashTwo,
            "Delete list",
            MenuAction::DeleteList(id),
        ),
        keys.1,
    ))
}

/// The room's menu (the header's "more" button): only what applies to the room. Empty when nothing
/// does (Completed, Agent activity), and then the button isn't drawn.
pub fn room_menu(
    state: &State,
    room: RoomKey,
    show_completed: bool,
    confirm: bool,
) -> Vec<MenuEntry> {
    if confirm && let RoomKey::List(id) = room {
        return delete_list_confirm(state, id, ("room-delete-confirm", "room-delete-cancel"))
            .into_iter()
            .collect();
    }
    let mut entries = Vec::new();
    match room {
        RoomKey::List(id) if state.is_live(id.into()) => {
            entries.push(item(
                "room-rename",
                Icon::Pencil,
                "Rename list",
                MenuAction::RenameList(id),
            ));
        }
        RoomKey::Collection(id) if state.is_live(id.into()) => {
            entries.push(item(
                "room-rename",
                Icon::Pencil,
                "Rename collection",
                MenuAction::RenameCollection(id),
            ));
        }
        _ => {}
    }
    if has_completed_section(room) {
        entries.push(completed_item(show_completed));
    }
    if let RoomKey::List(id) = room
        && state.is_live(id.into())
    {
        entries.push(MenuEntry::Separator);
        entries.push(danger(
            "room-delete",
            Icon::TrashTwo,
            "Delete list…",
            MenuAction::AskDeleteList(id),
        ));
    }
    entries
}

/// A list's context menu in the sidebar.
pub fn list_menu(state: &State, id: ListId, confirm: bool) -> Vec<MenuEntry> {
    if !state.is_live(id.into()) {
        return Vec::new();
    }
    if confirm {
        return delete_list_confirm(state, id, ("list-delete-confirm", "list-delete-cancel"))
            .into_iter()
            .collect();
    }
    vec![
        item(
            "list-rename",
            Icon::Pencil,
            "Rename",
            MenuAction::RenameList(id),
        ),
        item(
            "list-up",
            Icon::ChevronUp,
            "Move up",
            MenuAction::Nudge(id.into(), -1),
        ),
        item(
            "list-down",
            Icon::ChevronDown,
            "Move down",
            MenuAction::Nudge(id.into(), 1),
        ),
        MenuEntry::Separator,
        danger(
            "list-delete",
            Icon::TrashTwo,
            "Delete list…",
            MenuAction::AskDeleteList(id),
        ),
    ]
}

/// A collection's context menu in the sidebar.
pub fn collection_menu(state: &State, id: CollectionId, collapsed: bool) -> Vec<MenuEntry> {
    if !state.is_live(id.into()) {
        return Vec::new();
    }
    let (icon, label) = if collapsed {
        (Icon::ChevronDown, "Expand")
    } else {
        (Icon::ChevronRight, "Collapse")
    };
    vec![
        item(
            "collection-toggle",
            icon,
            label,
            MenuAction::ToggleCollection(id),
        ),
        item(
            "collection-new-list",
            Icon::ListPlus,
            "New list here",
            MenuAction::NewListIn(id),
        ),
        item(
            "collection-rename",
            Icon::Pencil,
            "Rename",
            MenuAction::RenameCollection(id),
        ),
        item(
            "collection-up",
            Icon::ChevronUp,
            "Move up",
            MenuAction::Nudge(id.into(), -1),
        ),
        item(
            "collection-down",
            Icon::ChevronDown,
            "Move down",
            MenuAction::Nudge(id.into(), 1),
        ),
    ]
}

/// The account menu (the sidebar footer's gear): the server the app is paired with, its admin pages,
/// Sync now and Sign out. Without a server (the sample data) only Sign out is offered. Once Sign out was
/// chosen, the menu is its confirmation.
pub fn account_menu(server: Option<&str>, confirming: bool) -> Vec<MenuEntry> {
    if confirming {
        return vec![confirm(
            "Sign out and remove this computer's copy of your tasks? They stay on the server;              changes not synced yet are lost."
                .into(),
            danger(
                "sign-out-confirm",
                Icon::LogOut,
                "Sign out",
                MenuAction::SignOut,
            ),
            "sign-out-cancel",
        )];
    }
    let mut entries = vec![
        MenuEntry::Caption {
            label: "Signed in to",
            value: server.map_or_else(
                || "Sample data (not signed in)".into(),
                |s| SharedString::from(s.to_string()),
            ),
        },
        MenuEntry::Separator,
    ];
    if server.and_then(admin_url).is_some() {
        entries.push(item(
            "open-admin",
            Icon::ExternalLink,
            "Open admin pages",
            MenuAction::OpenAdmin,
        ));
        entries.push(item(
            "sync-now",
            Icon::RefreshCw,
            "Sync now",
            MenuAction::SyncNow,
        ));
        entries.push(MenuEntry::Separator);
    }
    entries.push(item(
        "sign-out",
        Icon::LogOut,
        "Sign out…",
        MenuAction::AskSignOut,
    ));
    entries
}

/// A priority's name, and its marks on a row.
pub fn priority_label(p: Priority) -> (&'static str, &'static str) {
    match p {
        Priority::High => ("High", "!!!"),
        Priority::Normal => ("Normal", "!!"),
        Priority::Low => ("Low", "!"),
        Priority::None => ("None", ""),
    }
}

/// The detail pane's priority picker: all four levels, highest first, the current one ticked.
pub fn priority_menu(task: TaskId, current: Priority) -> Vec<MenuEntry> {
    [
        ("priority-high", Priority::High),
        ("priority-normal", Priority::Normal),
        ("priority-low", Priority::Low),
        ("priority-none", Priority::None),
    ]
    .into_iter()
    .map(|(key, level)| {
        let (label, marks) = priority_label(level);
        MenuEntry::Item(MenuItem {
            key: key.into(),
            icon: Icon::Flag,
            glyph: None,
            label: label.into(),
            danger: false,
            checked: level == current,
            hint: (!marks.is_empty()).then(|| marks.into()),
            action: MenuAction::SetPriority(task, level),
        })
    })
    .collect()
}

/// A task's context menu (right-click a row): what its row and the list's keys already do (complete or
/// reopen, My Day, Important, the priority picker), then Move to… and Delete. Deleting doesn't ask:
/// Undo brings the task back. Empty once the task is gone.
pub fn task_menu(state: &State, task: TaskId, today: Date) -> Vec<MenuEntry> {
    let Some(t) = state
        .tasks
        .get(&task)
        .filter(|_| state.is_live(task.into()))
    else {
        return Vec::new();
    };
    let done = if t.is_done() { "Reopen" } else { "Complete" };
    let my_day = if t.my_day == Some(today) {
        "Remove from My Day"
    } else {
        "Add to My Day"
    };
    let important = if t.priority == Priority::High {
        "Remove importance"
    } else {
        "Mark as important"
    };
    let marks = priority_label(t.priority).1;
    let priority = match item(
        "task-priority",
        Icon::Flag,
        "Priority…",
        MenuAction::AskPriority(task),
    ) {
        MenuEntry::Item(i) => MenuEntry::Item(MenuItem {
            hint: (!marks.is_empty()).then(|| marks.into()),
            ..i
        }),
        other => other,
    };
    vec![
        item(
            "task-done",
            Icon::CircleCheck,
            done,
            MenuAction::ToggleDone(task),
        ),
        item(
            "task-my-day",
            Icon::Sun,
            my_day,
            MenuAction::ToggleMyDay(task),
        ),
        item(
            "task-important",
            Icon::Star,
            important,
            MenuAction::ToggleImportant(task),
        ),
        priority,
        item(
            "task-move",
            Icon::List,
            "Move to…",
            MenuAction::AskMove(task),
        ),
        MenuEntry::Separator,
        danger(
            "task-delete",
            Icon::TrashTwo,
            "Delete task",
            MenuAction::DeleteTask(task),
        ),
    ]
}

/// Whether `filter` (typed into the list picker) finds a list by its name or its collection's: every
/// word of it, ignoring case and accents.
fn matches_filter(filter: &str, list: &str, collection: Option<&str>) -> bool {
    let haystack = fold(&format!("{list} {}", collection.unwrap_or_default()));
    fold(filter)
        .split_whitespace()
        .all(|w| haystack.contains(w))
}

/// The list picker that moves a task (Move to…): the lists grouped as the sidebar shows them, each
/// collection's under its name, the task's own list ticked. What's typed (`filter`) narrows them by
/// name or collection. Empty once the task is gone.
pub fn move_menu(state: &State, task: TaskId, filter: &str) -> Vec<MenuEntry> {
    let Some(t) = state
        .tasks
        .get(&task)
        .filter(|_| state.is_live(task.into()))
    else {
        return Vec::new();
    };
    let groups = commands::list_groups(state);
    let with_collections = groups.iter().any(|(c, _)| c.is_some());
    let mut entries = vec![MenuEntry::Filter(filter.to_string().into())];
    let mut found = false;
    for (collection, ids) in groups {
        let collection = collection.and_then(|c| state.collections.get(&c));
        let heading = match collection {
            Some(c) => Some(labels::one_line(&c.name)),
            None if with_collections => Some("Other lists".to_string()),
            None => None,
        };
        let items: Vec<MenuEntry> = ids
            .iter()
            .filter_map(|id| state.lists.get(id))
            .filter(|l| matches_filter(filter, &l.name, collection.map(|c| c.name.as_str())))
            .map(|l| {
                MenuEntry::Item(MenuItem {
                    key: format!("move-{}", l.id).into(),
                    icon: Icon::List,
                    glyph: Some((
                        lists::list_glyph(&l.name),
                        lists::hue(l.color.as_deref(), l.id.as_uuid().as_bytes()),
                    )),
                    label: labels::one_line(&l.name).into(),
                    danger: false,
                    checked: l.id == t.list,
                    hint: None,
                    action: MenuAction::MoveTask(task, l.id),
                })
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        found = true;
        entries.extend(heading.map(|h| MenuEntry::Heading(h.into())));
        entries.extend(items);
    }
    if !found {
        let typed = labels::one_line(filter.trim());
        entries.push(MenuEntry::Note(
            format!("No list matches \u{201c}{typed}\u{201d}.").into(),
        ));
    }
    entries
}

/// The "couldn't sync" list (the sidebar's sync line, when something was refused): each refused change
/// in plain words, what it was about and why (never the server's own text), beside its Dismiss
/// button, then Dismiss all.
pub fn failures_menu(state: &State, failed: &[FailedEntry]) -> Vec<MenuEntry> {
    use todo_client::failure;
    if failed.is_empty() {
        return Vec::new();
    }
    let heading = match failed.len() {
        1 => "1 change couldn't sync".to_string(),
        n => format!("{n} changes couldn't sync"),
    };
    let mut entries = vec![MenuEntry::Heading(heading.into())];
    for f in failed {
        let what = failure::what(&f.entry);
        let what = match failure::name(&f.entry, state) {
            Some(name) => format!("{what} \u{201c}{}\u{201d}", labels::one_line(name)),
            None => what.to_string(),
        };
        let id = f.entry.command_id;
        entries.push(MenuEntry::Failure {
            what: what.into(),
            why: failure::why(f).into(),
            dismiss: MenuItem {
                key: format!("dismiss-{id}").into(),
                icon: Icon::X,
                glyph: None,
                label: "Dismiss".into(),
                danger: false,
                checked: false,
                hint: None,
                action: MenuAction::DismissFailure(id),
            },
        });
    }
    if failed.len() > 1 {
        entries.push(MenuEntry::Separator);
        entries.push(item(
            "dismiss-all",
            Icon::X,
            "Dismiss all",
            MenuAction::DismissAllFailures,
        ));
    }
    entries
}

/// The server's admin pages, `<server>/admin`, built from the address the app was paired with (checked
/// like the transport checks it: https, or plain http only for this computer). Never a built-in host.
pub fn admin_url(server: &str) -> Option<String> {
    let origin = todo_client::native::server_origin(server).ok()?;
    Some(format!("{}/admin", origin.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::Smart;

    fn actions(entries: &[MenuEntry]) -> Vec<MenuAction> {
        items(entries).map(|i| i.action).collect()
    }

    fn list_named(state: &State, name: &str) -> ListId {
        state
            .lists
            .values()
            .find(|l| l.name == name)
            .map(|l| l.id)
            .expect("list")
    }

    #[test]
    fn a_lists_room_menu_renames_toggles_completed_and_deletes() {
        let f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        let menu = room_menu(&f.state, RoomKey::List(office), false, false);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::RenameList(office),
                MenuAction::ToggleCompleted,
                MenuAction::AskDeleteList(office),
            ]
        );
        assert!(items(&menu).last().is_some_and(|i| i.danger));
        // Confirming says, in full, what goes with it, above short buttons: Delete list and Cancel.
        let confirm = room_menu(&f.state, RoomKey::List(office), false, true);
        assert_eq!(
            actions(&confirm),
            [MenuAction::DeleteList(office), MenuAction::Cancel]
        );
        assert!(is_confirmation(&confirm));
        let [MenuEntry::Confirm { message, buttons }] = confirm.as_slice() else {
            panic!("{confirm:?}");
        };
        assert!(
            message.starts_with("Delete the list “Office” and its "),
            "{message}"
        );
        assert!(message.ends_with("Undo brings them back."), "{message}");
        let labels: Vec<&str> = buttons.iter().map(|b| b.label.as_ref()).collect();
        assert_eq!(labels, ["Delete list", "Cancel"]);
        assert!(buttons[0].danger && !buttons[1].danger);
        // From the keyboard it starts on Cancel: a second Enter never deletes.
        assert_eq!(first_highlight(&confirm), Some(1));
    }

    #[test]
    fn the_confirmation_names_the_whole_list_and_counts_its_tasks() {
        let mut f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        let long = "Quarterly planning with the leadership team and every regional office";
        f.state.lists.get_mut(&office).expect("list").name = format!(
            "{long}
second line"
        );
        let open = f
            .state
            .tasks
            .values()
            .filter(|t| t.list == office && t.deleted.is_none())
            .count();
        let menu = list_menu(&f.state, office, true);
        let [MenuEntry::Confirm { message, .. }] = menu.as_slice() else {
            panic!("{menu:?}");
        };
        // Never cut short: the whole name (on one line) and the count are in the message.
        assert!(
            message.contains(&format!("“{long} second line”")),
            "{message}"
        );
        assert!(message.contains(&format!("its {open} tasks?")), "{message}");
        // A list without tasks says so, and Undo brings it (not "them") back.
        for task in f.state.tasks.values_mut().filter(|t| t.list == office) {
            task.deleted = Some(todo_core::model::Tombstone {
                at: f.now.at,
                by: todo_core::Actor::System,
            });
        }
        let menu = list_menu(&f.state, office, true);
        let [MenuEntry::Confirm { message, .. }] = menu.as_slice() else {
            panic!("{menu:?}");
        };
        assert!(
            message.ends_with("second line”? Undo brings it back."),
            "{message}"
        );
    }

    #[test]
    fn only_items_that_open_something_of_their_own_keep_the_keyboard() {
        let list = ListId::from_uuid(uuid::Uuid::nil());
        let col = CollectionId::from_uuid(uuid::Uuid::nil());
        let task = TaskId::from_uuid(uuid::Uuid::nil());
        for action in [
            MenuAction::RenameList(list),
            MenuAction::RenameCollection(col),
            MenuAction::NewListIn(col),
            MenuAction::DeleteList(list),
            MenuAction::SignOut,
        ] {
            assert!(action.moves_focus(), "{action:?}");
        }
        // These give the keyboard back to the menu's button.
        for action in [
            MenuAction::ToggleCompleted,
            MenuAction::Nudge(list.into(), 1),
            MenuAction::ToggleCollection(col),
            MenuAction::OpenAdmin,
            MenuAction::SyncNow,
            MenuAction::Cancel,
            MenuAction::SetPriority(task, Priority::High),
            MenuAction::AskDeleteList(list),
            MenuAction::AskSignOut,
            MenuAction::ToggleDone(task),
            MenuAction::ToggleMyDay(task),
            MenuAction::ToggleImportant(task),
            MenuAction::AskPriority(task),
            MenuAction::AskMove(task),
            MenuAction::MoveTask(task, list),
            MenuAction::DeleteTask(task),
            MenuAction::DismissFailure(uuid::Uuid::nil()),
            MenuAction::DismissAllFailures,
        ] {
            assert!(!action.moves_focus(), "{action:?}");
        }
    }

    #[test]
    fn smart_rooms_offer_only_what_applies() {
        let f = fixtures::my_day();
        for smart in Smart::ALL {
            let menu = room_menu(&f.state, RoomKey::Smart(smart), false, false);
            let expected: &[MenuAction] = match smart {
                Smart::Completed | Smart::AgentActivity => &[],
                _ => &[MenuAction::ToggleCompleted],
            };
            assert_eq!(actions(&menu), expected, "{smart:?}");
            // A smart list can't be deleted, even when asked to confirm.
            assert!(
                !actions(&room_menu(&f.state, RoomKey::Smart(smart), false, true))
                    .iter()
                    .any(|a| matches!(a, MenuAction::DeleteList(_)))
            );
        }
    }

    #[test]
    fn a_collections_room_menu_renames_it_and_never_deletes() {
        let f = fixtures::work();
        let (&id, _) = f.state.collections.iter().next().expect("collection");
        let menu = room_menu(&f.state, RoomKey::Collection(id), true, false);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::RenameCollection(id),
                MenuAction::ToggleCompleted
            ]
        );
        assert_eq!(items(&menu).nth(1).expect("toggle").label, "Hide completed");
    }

    #[test]
    fn a_deleted_list_offers_nothing() {
        let mut f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        f.state.lists.get_mut(&office).expect("list").deleted = Some(todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        });
        assert!(list_menu(&f.state, office, false).is_empty());
        assert!(list_menu(&f.state, office, true).is_empty());
        assert!(room_menu(&f.state, RoomKey::List(office), false, true).is_empty());
    }

    #[test]
    fn the_account_menu_links_the_paired_servers_admin_pages() {
        let menu = account_menu(Some("https://todo.example.com"), false);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::OpenAdmin,
                MenuAction::SyncNow,
                MenuAction::AskSignOut
            ]
        );
        assert_eq!(
            admin_url("https://todo.example.com").as_deref(),
            Some("https://todo.example.com/admin")
        );
        assert_eq!(
            admin_url("https://todo.example.com/").as_deref(),
            Some("https://todo.example.com/admin")
        );
        assert_eq!(
            admin_url("http://localhost:8080").as_deref(),
            Some("http://localhost:8080/admin")
        );
        // Never a cleartext remote server or anything but a web address.
        assert_eq!(admin_url("http://todo.example.com"), None);
        assert_eq!(admin_url("javascript:alert(1)"), None);
        assert_eq!(admin_url("file:///C:/Windows"), None);
        // The sample data has no server: only Sign out.
        let sample = account_menu(None, false);
        assert_eq!(actions(&sample), [MenuAction::AskSignOut]);
        // Confirming, the menu is only the question and its two buttons.
        let confirm = account_menu(Some("https://todo.example.com"), true);
        assert_eq!(actions(&confirm), [MenuAction::SignOut, MenuAction::Cancel]);
        let [MenuEntry::Confirm { message, buttons }] = confirm.as_slice() else {
            panic!("{confirm:?}");
        };
        assert!(message.contains("remove this computer's copy"), "{message}");
        assert_eq!(buttons[0].label, "Sign out");
    }

    #[test]
    fn the_priority_picker_offers_all_four_levels_and_ticks_the_current_one() {
        let task = TaskId::from_uuid(uuid::Uuid::nil());
        let menu = priority_menu(task, Priority::Low);
        let levels: Vec<Priority> = items(&menu)
            .map(|i| match i.action {
                MenuAction::SetPriority(_, p) => p,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            levels,
            [
                Priority::High,
                Priority::Normal,
                Priority::Low,
                Priority::None
            ]
        );
        let ticked: Vec<&str> = items(&menu)
            .filter(|i| i.checked)
            .map(|i| i.label.as_ref())
            .collect();
        assert_eq!(ticked, ["Low"]);
        // From the keyboard it starts on the current level.
        assert_eq!(first_highlight(&menu), Some(2));
        let none = room_menu(
            &fixtures::my_day().state,
            RoomKey::Smart(Smart::MyDay),
            false,
            false,
        );
        assert_eq!(first_highlight(&none), Some(0));
        assert_eq!(first_highlight(&[]), None);
    }

    #[test]
    fn the_keyboard_highlight_wraps() {
        assert_eq!(step_highlight(None, 1, 3), Some(0));
        assert_eq!(step_highlight(None, -1, 3), Some(2));
        assert_eq!(step_highlight(Some(2), 1, 3), Some(0));
        assert_eq!(step_highlight(Some(0), -1, 3), Some(2));
        assert_eq!(step_highlight(Some(1), 1, 3), Some(2));
        assert_eq!(step_highlight(None, 1, 0), None);
    }

    #[test]
    fn item_keys_are_unique_within_each_menu() {
        let f = fixtures::work();
        let list = *f.state.lists.keys().next().expect("list");
        let col = *f.state.collections.keys().next().expect("collection");
        let task = *f.state.tasks.keys().next().expect("task");
        let menus = [
            room_menu(&f.state, RoomKey::List(list), false, false),
            list_menu(&f.state, list, false),
            collection_menu(&f.state, col, false),
            account_menu(Some("https://todo.example.com"), false),
            account_menu(Some("https://todo.example.com"), true),
            list_menu(&f.state, list, true),
            room_menu(&f.state, RoomKey::List(list), false, true),
            priority_menu(task, Priority::None),
            task_menu(&f.state, task, f.now.today()),
            move_menu(&f.state, task, ""),
            failures_menu(&f.state, &failed(&f.state, 3)),
        ];
        for menu in menus {
            let keys: Vec<&str> = items(&menu).map(|i| i.key.as_ref()).collect();
            let unique: std::collections::BTreeSet<&str> = keys.iter().copied().collect();
            assert_eq!(keys.len(), unique.len(), "{keys:?}");
        }
    }

    /// `n` refused changes for the "couldn't sync" list (`fixtures::failed_changes`).
    fn failed(state: &State, n: usize) -> Vec<FailedEntry> {
        crate::fixtures::failed_changes(state)
            .into_iter()
            .take(n)
            .collect()
    }

    #[test]
    fn a_tasks_menu_offers_what_its_row_does_then_move_and_delete() {
        let f = fixtures::my_day();
        let today = f.now.today();
        let open = f
            .state
            .tasks
            .values()
            .find(|t| !t.is_done() && f.state.is_live(t.id.into()) && t.my_day == Some(today))
            .expect("an open task in My Day");
        let id = open.id;
        let menu = task_menu(&f.state, id, today);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::ToggleDone(id),
                MenuAction::ToggleMyDay(id),
                MenuAction::ToggleImportant(id),
                MenuAction::AskPriority(id),
                MenuAction::AskMove(id),
                MenuAction::DeleteTask(id),
            ]
        );
        let labels: Vec<&str> = items(&menu).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels[..2], ["Complete", "Remove from My Day"]);
        // Delete is last, apart, in the danger colour, and doesn't ask: Undo covers it.
        assert!(matches!(menu[menu.len() - 2], MenuEntry::Separator));
        assert!(items(&menu).last().is_some_and(|i| i.danger));
        assert!(!is_confirmation(&menu));
        // A done task reopens.
        let done = f
            .state
            .tasks
            .values()
            .find(|t| t.is_done() && f.state.is_live(t.id.into()))
            .expect("a done task");
        let labels: Vec<SharedString> = items(&task_menu(&f.state, done.id, today))
            .map(|i| i.label.clone())
            .collect();
        assert_eq!(labels[0], "Reopen");
        // Gone, it offers nothing.
        let mut gone = f.state.clone();
        gone.tasks.get_mut(&id).expect("task").deleted = Some(todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        });
        assert!(task_menu(&gone, id, today).is_empty());
        assert!(move_menu(&gone, id, "").is_empty());
    }

    #[test]
    fn the_list_picker_groups_lists_by_collection_ticks_the_tasks_own_and_filters() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let own = f.state.tasks[&task].list;
        let menu = move_menu(&f.state, task, "");
        // The filter line first, then each collection's name above its lists, as in the sidebar.
        assert!(matches!(&menu[0], MenuEntry::Filter(t) if t.is_empty()));
        let headings: Vec<&str> = menu
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Heading(h) => Some(h.as_ref()),
                _ => None,
            })
            .collect();
        let shell = f.shell();
        let mut expected: Vec<&str> = shell
            .sidebar
            .collections
            .iter()
            .map(|c| c.name.as_ref())
            .collect();
        if !shell.sidebar.lists.is_empty() {
            expected.push("Other lists");
        }
        assert_eq!(headings, expected);
        let live = f
            .state
            .lists
            .values()
            .filter(|l| f.state.is_live(l.id.into()))
            .count();
        assert_eq!(items(&menu).count(), live, "every live list, once");
        let ticked: Vec<MenuAction> = items(&menu)
            .filter(|i| i.checked)
            .map(|i| i.action)
            .collect();
        assert_eq!(ticked, [MenuAction::MoveTask(task, own)]);
        // From the keyboard it starts on the task's own list.
        let at = first_highlight(&menu).expect("highlight");
        assert_eq!(
            items(&menu).nth(at).map(|i| i.action),
            Some(MenuAction::MoveTask(task, own))
        );
        assert!(items(&menu).all(|i| i.glyph.is_some()));
        // The keyboard scrolls the lists, under the filter line that stays put: the first list is the
        // second entry that scrolls (after its collection's name), the last one the last.
        assert_eq!(child_of_item(&menu, 0), Some(1));
        assert_eq!(
            child_of_item(&menu, live - 1),
            Some(scrolling(&menu).count() - 1)
        );
        assert_eq!(child_of_item(&menu, live), None);

        // Typing narrows it, by list or collection name, ignoring case, and drops empty collections.
        let errands = list_named(&f.state, "Errands");
        let narrowed = move_menu(&f.state, task, "ERRAND");
        assert_eq!(actions(&narrowed), [MenuAction::MoveTask(task, errands)]);
        assert!(matches!(&narrowed[0], MenuEntry::Filter(t) if t == "ERRAND"));
        let work = shell
            .sidebar
            .collections
            .iter()
            .find(|c| c.lists.len() > 1)
            .expect("a collection with lists");
        let by_collection = move_menu(&f.state, task, &work.name.to_lowercase());
        assert!(items(&by_collection).count() >= work.lists.len());
        let nothing = move_menu(&f.state, task, "zzz");
        assert_eq!(items(&nothing).count(), 0);
        assert!(matches!(nothing.last(), Some(MenuEntry::Note(n)) if n.contains("zzz")));
    }

    #[test]
    fn the_couldnt_sync_list_names_each_change_and_says_why_in_plain_words() {
        let f = fixtures::my_day();
        let failed = failed(&f.state, 3);
        let menu = failures_menu(&f.state, &failed);
        assert!(matches!(&menu[0], MenuEntry::Heading(h) if h == "3 changes couldn't sync"));
        let rows: Vec<(&str, &str)> = menu
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Failure { what, why, .. } => Some((what.as_ref(), why.as_ref())),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 3);
        for (what, why) in &rows {
            // The fixture's server messages say "raw"; they never show.
            assert!(
                !what.contains("raw") && !why.contains("raw"),
                "{what} {why}"
            );
        }
        assert!(
            rows.iter().all(|(what, _)| what.contains('\u{201c}')),
            "{rows:?}"
        );
        // Each has its own Dismiss, then Dismiss all.
        let mut expected: Vec<MenuAction> = failed
            .iter()
            .map(|e| MenuAction::DismissFailure(e.entry.command_id))
            .collect();
        expected.push(MenuAction::DismissAllFailures);
        assert_eq!(actions(&menu), expected);
        assert!(is_wide(&menu) && !is_confirmation(&menu));
        // One refused change needs no Dismiss all; none, no list.
        let one = failures_menu(&f.state, &failed[..1]);
        assert!(matches!(&one[0], MenuEntry::Heading(h) if h == "1 change couldn't sync"));
        assert_eq!(items(&one).count(), 1);
        assert!(failures_menu(&f.state, &[]).is_empty());
    }
}
