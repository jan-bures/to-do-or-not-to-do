//! What each popover menu offers, as data: the room's menu (the header's "more" button), a list's or a
//! collection's context menu, the account menu and the detail pane's priority picker. The view draws the
//! entries and runs an item's [`MenuAction`]; the keyboard walks the same items, so every menu behaves
//! alike. Pure, so what a menu offers in each room is unit-tested.

use gpui::SharedString;
use todo_core::State;
use todo_core::ids::{CollectionId, EntityId, ListId, TaskId};
use todo_core::model::Priority;

use super::{RoomKey, labels};
use crate::theme::Icon;

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
    pub key: &'static str,
    pub icon: Icon,
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
}

impl MenuEntry {
    /// The items it offers, in the order the keyboard walks them.
    pub fn items(&self) -> &[MenuItem] {
        match self {
            Self::Item(item) => std::slice::from_ref(item),
            Self::Confirm { buttons, .. } => buttons,
            Self::Separator | Self::Caption { .. } => &[],
        }
    }
}

fn item(
    key: &'static str,
    icon: Icon,
    label: impl Into<SharedString>,
    action: MenuAction,
) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        key,
        icon,
        label: label.into(),
        danger: false,
        checked: false,
        hint: None,
        action,
    })
}

fn danger(
    key: &'static str,
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

/// Whether the menu is a confirmation (drawn wider, its buttons side by side).
pub fn is_confirmation(entries: &[MenuEntry]) -> bool {
    entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Confirm { .. }))
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
            key,
            icon: Icon::Flag,
            label: label.into(),
            danger: false,
            checked: level == current,
            hint: (!marks.is_empty()).then(|| marks.into()),
            action: MenuAction::SetPriority(task, level),
        })
    })
    .collect()
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
        ];
        for menu in menus {
            let keys: Vec<&str> = items(&menu).map(|i| i.key).collect();
            let unique: std::collections::BTreeSet<&str> = keys.iter().copied().collect();
            assert_eq!(keys.len(), unique.len(), "{keys:?}");
        }
    }
}
