//! What each popover menu offers, as data: the room's menu (the header's "more" button), a list's, a
//! collection's or a task's context menu, the account menu, the priority picker, the list picker that
//! moves a task, the collection picker that moves a list, the colour swatches, the tag picker, a task's
//! links, and the "couldn't sync" list. The view draws the entries and runs an item's
//! [`MenuAction`]; the keyboard walks the same items, so every menu behaves alike. Pure, so what a menu
//! offers in each room is unit-tested.

pub mod reminder;
pub mod repeat;
use gpui::SharedString;
use todo_client::CommandId;
use todo_client::intent::merge;
use todo_client::outbox::FailedEntry;
use todo_core::State;
use todo_core::ids::{CollectionId, EntityId, LinkId, ListId, TagId, TaskId};
use todo_core::model::Priority;
use todo_core::query::fold;
use todo_core::time::Date;

use super::{AgentKey, AgentRow, RoomKey, commands, labels, lists};
use crate::theme::{Glyph, Hue, Icon, hues};

/// What a list's, a collection's or the room's menu shows: its items, or in their place the delete
/// confirmation, the collection picker that moves a list, or the colour swatches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Main,
    Confirm,
    Move,
    Colour,
}

/// What the tag picker shows: every tag to put on the task or take off, one tag's own page (rename,
/// recolour), or that tag's colour swatches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagPage {
    Pick,
    Edit(TagId),
    Colour(TagId),
    Confirm(TagId),
}

/// How many colour swatches a row of the colour page holds: Up and Down move a row at a time.
pub const SWATCH_COLUMNS: usize = 6;

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
    NewList,
    NewCollection,
    NewListIn(CollectionId),
    /// Open the server's admin pages in the browser.
    OpenAdmin,
    /// Open the device settings dialog.
    DeviceSettings,
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
    PickMerge(TaskId),
    AskMerge(TaskId, TaskId),
    Merge(TaskId, TaskId),
    /// Deletes the task, without asking: Undo brings it back.
    DeleteTask(TaskId),
    /// Ask before deleting a collection: the menu turns into the confirmation.
    AskDeleteCollection(CollectionId),
    /// Delete the collection, its lists and their tasks with it (Undo brings them back).
    DeleteCollection(CollectionId),
    /// Turns a list's menu into the collection picker, in place.
    AskMoveList(ListId),
    /// Moves the list into the collection, or out of any (nothing when it's there already).
    MoveList(ListId, Option<CollectionId>),
    /// Turns the menu into the colour swatches of a collection, list or tag, in place.
    AskColour(EntityId),
    /// Recolours a collection, list or tag with the swatch of this key.
    SetColour(EntityId, &'static str),
    /// Puts the tag on the task, or takes it off. The picker stays open.
    ToggleTag(TaskId, TagId),
    /// Creates a tag named as typed into the picker and puts it on the task.
    CreateTag(TaskId),
    /// Turns the tag picker into the tag's own page (rename, colour).
    EditTag(TaskId, TagId),
    /// Back from a tag's own page to every tag.
    AllTags(TaskId),
    /// Renames a tag in place, in the detail pane's Tags row.
    RenameTag(TagId),
    AskDeleteTag(TagId),
    DeleteTag(TagId),
    Repeat(TaskId, repeat::Action),
    /// The reminder picker: a preset, its custom page, a step on it, Set reminder or Remove.
    Reminder(TaskId, reminder::Action),
    /// Turns a task's menu into its links: add one, or remove one.
    AskLinks(TaskId),
    /// Puts the keyboard in the detail pane's link field, for the task.
    AddLink(TaskId),
    /// Removes a link from its task, without asking: Undo brings it back.
    RemoveLink(LinkId),
    /// Takes one refused change off the "couldn't sync" list.
    DismissFailure(CommandId),
    DismissAllFailures,
    /// Reads the newest changes again (the Activity room).
    RefreshActivity,
    /// Shows every change, or (`Some`) one agent's, in the Activity room.
    FilterActivity(Option<AgentKey>),
}

impl MenuAction {
    /// Whether choosing it takes the keyboard somewhere of its own: an editor opens (renaming, a new
    /// list, the link field), the room it was opened from goes away (deleting the list or collection),
    /// or the whole window changes (signing out). Any other item chosen from the keyboard gives the
    /// keyboard back to the menu's button.
    pub fn moves_focus(self) -> bool {
        matches!(
            self,
            Self::RenameList(_)
                | Self::RenameCollection(_)
                | Self::RenameTag(_)
                | Self::NewList
                | Self::NewCollection
                | Self::NewListIn(_)
                | Self::DeleteList(_)
                | Self::DeleteCollection(_)
                | Self::AddLink(_)
                | Self::SignOut
                | Self::DeviceSettings
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
    /// A colour disc, drawn instead of the icon: a tag's colour, a swatch, or the colour Colour… changes.
    pub dot: Option<Hue>,
    pub label: SharedString,
    /// Destructive: drawn in the danger colour.
    pub danger: bool,
    /// The current choice (a tick on the right).
    pub checked: bool,
    /// Quiet text on the right, such as the priority's marks.
    pub hint: Option<SharedString>,
    pub action: MenuAction,
    /// A second action, behind a chevron on the right (and the Right key): a tag's own page.
    pub more: Option<MenuAction>,
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
        /// Why it's asked again: what it's about changed while it was open ([`on_confirm`]).
        note: Option<SharedString>,
        buttons: [MenuItem; 2],
    },
    /// A quiet section label: a collection's name in the list picker.
    Heading(SharedString),
    /// What has been typed to narrow a picker, or (nothing typed yet) a hint that typing narrows it.
    Filter {
        typed: SharedString,
        hint: &'static str,
    },
    /// Colour swatches, side by side in rows of [`SWATCH_COLUMNS`]: each a disc in its colour, the
    /// current one ticked.
    Swatches(Vec<MenuItem>),
    /// A labelled group of compact choices in the custom repeat editor. A stepper puts its
    /// numeric value between its two buttons.
    Controls {
        label: &'static str,
        value: Option<String>,
        buttons: Vec<MenuItem>,
    },
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
            Self::Swatches(items) | Self::Controls { buttons: items, .. } => items,
            Self::Separator
            | Self::Caption { .. }
            | Self::Heading(_)
            | Self::Filter { .. }
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
        dot: None,
        label: label.into(),
        danger: false,
        checked: false,
        hint: None,
        action,
        more: None,
    })
}

/// `entry` (an item) with `f` applied to its item.
fn with(entry: MenuEntry, f: impl FnOnce(MenuItem) -> MenuItem) -> MenuEntry {
    match entry {
        MenuEntry::Item(i) => MenuEntry::Item(f(i)),
        other => other,
    }
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

/// The pinned sidebar button's choices. Neither has a dedicated keyboard shortcut.
pub fn new_menu() -> Vec<MenuEntry> {
    vec![
        item("create-list", Icon::Plus, "List", MenuAction::NewList),
        item(
            "create-collection",
            Icon::FolderPlus,
            "Collection",
            MenuAction::NewCollection,
        ),
    ]
}

/// The items of `entries`, in order: what the keyboard walks.
pub fn items(entries: &[MenuEntry]) -> impl Iterator<Item = &MenuItem> {
    entries.iter().flat_map(MenuEntry::items)
}

/// The entries that scroll in a long menu: all but a picker's filter line, which stays put above them.
pub fn scrolling(entries: &[MenuEntry]) -> impl Iterator<Item = &MenuEntry> {
    entries
        .iter()
        .filter(|e| !matches!(e, MenuEntry::Filter { .. }))
}

/// Whether the menu is the colour swatches, which the arrows walk as a grid.
pub fn is_grid(entries: &[MenuEntry]) -> bool {
    entries.iter().any(|e| matches!(e, MenuEntry::Swatches(_)))
}

/// The keyboard highlight after an arrow key in the swatch grid of `count` items, [`SWATCH_COLUMNS`] to
/// a row: Left and Right walk them in order and wrap round; Up and Down move a row, staying put at the
/// top or bottom row. The first press lands on the first (Down, Right) or last (Up, Left) swatch.
pub fn grid_step(current: Option<usize>, key: &str, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(at) = current else {
        return Some(if matches!(key, "up" | "left") {
            count - 1
        } else {
            0
        });
    };
    Some(match key {
        "left" => (at + count - 1) % count,
        "right" => (at + 1) % count,
        "up" => at.checked_sub(SWATCH_COLUMNS).unwrap_or(at),
        "down" if at + SWATCH_COLUMNS < count => at + SWATCH_COLUMNS,
        _ => at,
    })
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

/// Whether the menu is drawn at the confirmation's width: a confirmation, the "couldn't sync" list,
/// whose reasons wrap beside their buttons, a page of steppers, or the reminder picker, whose presets
/// say when beside their names.
pub fn is_wide(entries: &[MenuEntry]) -> bool {
    entries.iter().any(|e| {
        matches!(
            e,
            MenuEntry::Confirm { .. } | MenuEntry::Failure { .. } | MenuEntry::Controls { .. }
        )
    }) || items(entries).any(|i| {
        matches!(
            i.action,
            MenuAction::Reminder(..) | MenuAction::AskMerge(..)
        )
    })
}

/// The key of item `i`, as the keyboard counts them. The keyboard's highlight is kept as the item's key,
/// never its place, so data that changes under the menu (a list added above, a new refusal) never moves
/// it to another item.
pub fn key_at(entries: &[MenuEntry], i: usize) -> Option<SharedString> {
    items(entries).nth(i).map(|item| item.key.clone())
}

/// Where the item with `key` is now, if it's still there.
pub fn index_of(entries: &[MenuEntry], key: &str) -> Option<usize> {
    items(entries).position(|item| item.key == key)
}

/// What Enter does on the highlighted item (`key`): its action, or nothing when that item has gone.
pub fn chosen(entries: &[MenuEntry], key: Option<&str>) -> Option<MenuAction> {
    let key = key?;
    items(entries)
        .find(|item| item.key == key)
        .map(|i| i.action)
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
        RoomKey::Activity(_) | RoomKey::Trash => false,
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
            note: None,
            buttons: [action, cancel],
        },
        (other, _) => other,
    }
}

/// A delete confirmation as the owner was asked it: the question (with the names and counts in it) as
/// it was drawn, and whether it was asked again because they changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub question: SharedString,
    pub changed: bool,
}

impl Asked {
    /// The question `entries` (a confirmation) ask now, asked for the first time.
    pub fn of(entries: &[MenuEntry]) -> Option<Self> {
        question(entries).map(|question| Self {
            question: question.clone(),
            changed: false,
        })
    }
}

/// The question a confirmation asks, in full.
pub fn question(entries: &[MenuEntry]) -> Option<&SharedString> {
    entries.iter().find_map(|e| match e {
        MenuEntry::Confirm { message, .. } => Some(message),
        _ => None,
    })
}

/// What confirming a delete does, given the question as it was asked and the confirmation the state asks
/// for now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    /// Nothing changed since it was asked: delete.
    Proceed,
    /// The names or counts changed (a sync, another device, an agent): delete nothing and ask this.
    AskAgain(SharedString),
    /// What it was about is gone.
    Gone,
}

/// Whether to go ahead with a confirmed delete. The confirmation always shows the question as it was
/// asked (`as_asked`), never one redrawn from data that changed under it; if what it would say now
/// differs, nothing is deleted and the new question is asked instead.
pub fn on_confirm(asked: Option<&Asked>, now: &[MenuEntry]) -> Confirmed {
    match (asked, question(now)) {
        (_, None) => Confirmed::Gone,
        (Some(asked), Some(q)) if &asked.question != q => Confirmed::AskAgain(q.clone()),
        _ => Confirmed::Proceed,
    }
}

/// `entries` (a confirmation) as the owner was asked: the question as it was drawn, with a note when it
/// was asked again because it changed. Its buttons are today's.
pub fn as_asked(entries: Vec<MenuEntry>, asked: &Asked) -> Vec<MenuEntry> {
    entries
        .into_iter()
        .map(|e| match e {
            MenuEntry::Confirm { buttons, .. } => {
                let what = match buttons[0].action {
                    MenuAction::DeleteCollection(_) => "collection",
                    MenuAction::DeleteTag(_) => "tag",
                    _ => "list",
                };
                MenuEntry::Confirm {
                    message: asked.question.clone(),
                    note: asked.changed.then(|| {
                        if matches!(buttons[0].action, MenuAction::Merge(..)) {
                            merge::CHANGED.into()
                        } else {
                            commands::delete_recheck_text(what).into()
                        }
                    }),
                    buttons,
                }
            }
            other => other,
        })
        .collect()
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

/// The confirmation that replaces a collection's menu once Delete was chosen. Deleting a collection hides
/// its lists and their tasks with it (the domain tombstones only the collection), so it says, in full,
/// how many of each go, and that Undo brings them all back.
fn delete_collection_confirm(
    state: &State,
    id: CollectionId,
    keys: (&'static str, &'static str),
) -> Option<MenuEntry> {
    Some(confirm(
        todo_client::intent::confirm::collection_question(state, id)?,
        danger(
            keys.0,
            Icon::TrashTwo,
            "Delete collection",
            MenuAction::DeleteCollection(id),
        ),
        keys.1,
    ))
}

/// The "Colour…" item: the colour disc it changes, beside the label.
fn colour_item(key: &'static str, id: EntityId, hue: Hue) -> MenuEntry {
    with(
        item(key, Icon::Pencil, "Colour…", MenuAction::AskColour(id)),
        |i| MenuItem {
            dot: Some(hue),
            ..i
        },
    )
}

/// A collection's, list's or tag's colour as drawn: its swatch (`lists::hue`).
fn colour_of(state: &State, id: EntityId) -> Option<Hue> {
    match id {
        EntityId::Collection(c) => {
            let c = state.collections.get(&c)?;
            Some(lists::hue(c.color.as_deref(), c.id.as_uuid().as_bytes()))
        }
        EntityId::List(l) => {
            let l = state.lists.get(&l)?;
            Some(lists::hue(l.color.as_deref(), l.id.as_uuid().as_bytes()))
        }
        EntityId::Tag(t) => {
            let t = state.tags.get(&t)?;
            Some(lists::hue(t.color.as_deref(), t.id.as_uuid().as_bytes()))
        }
        _ => None,
    }
    .filter(|_| state.is_live(id))
}

/// The colour page of a collection, list or tag: the design's swatches, the current one ticked. Empty
/// once it's gone.
pub fn colour_menu(state: &State, id: EntityId) -> Vec<MenuEntry> {
    let Some((current, heading)) = colour_of(state, id).zip(commands::colour_heading(id)) else {
        return Vec::new();
    };
    let swatches = hues::SWATCHES
        .iter()
        .map(|s| MenuItem {
            key: format!("colour-{}", s.key).into(),
            icon: Icon::Check,
            glyph: None,
            dot: Some(s.degrees),
            label: commands::swatch_name(s.key).into(),
            danger: false,
            checked: (s.degrees - current).abs() < 0.5,
            hint: None,
            action: MenuAction::SetColour(id, s.key),
            more: None,
        })
        .collect();
    vec![
        MenuEntry::Heading(heading.into()),
        MenuEntry::Swatches(swatches),
    ]
}

/// The room's menu (the header's "more" button): only what applies to the room. Empty when nothing
/// does (Completed, Agent activity), and then the button isn't drawn. `page` turns it into a delete
/// confirmation, the collection picker (a list's room) or the colour swatches.
pub fn room_menu(state: &State, room: RoomKey, show_completed: bool, page: Page) -> Vec<MenuEntry> {
    match (room, page) {
        (RoomKey::List(id), Page::Confirm) => {
            return delete_list_confirm(state, id, ("room-delete-confirm", "room-delete-cancel"))
                .into_iter()
                .collect();
        }
        (RoomKey::Collection(id), Page::Confirm) => {
            return delete_collection_confirm(
                state,
                id,
                ("room-delete-confirm", "room-delete-cancel"),
            )
            .into_iter()
            .collect();
        }
        (RoomKey::List(id), Page::Move) => return move_list_menu(state, id),
        (RoomKey::List(id), Page::Colour) => return colour_menu(state, id.into()),
        (RoomKey::Collection(id), Page::Colour) => return colour_menu(state, id.into()),
        (_, Page::Main) => {}
        // A smart list has none of these pages.
        _ => return Vec::new(),
    }
    let mut entries = Vec::new();
    let colour = colour_of(
        state,
        match room {
            RoomKey::List(id) => id.into(),
            RoomKey::Collection(id) => id.into(),
            RoomKey::Smart(_) | RoomKey::Activity(_) | RoomKey::Trash => {
                return completed_only(room, show_completed);
            }
        },
    );
    match room {
        RoomKey::List(id) if state.is_live(id.into()) => {
            entries.push(item(
                "room-rename",
                Icon::Pencil,
                "Rename list",
                MenuAction::RenameList(id),
            ));
            entries.extend(colour.map(|hue| colour_item("room-colour", id.into(), hue)));
            if commands::can_move_list(state, id) {
                entries.push(item(
                    "room-move",
                    Icon::List,
                    "Move to…",
                    MenuAction::AskMoveList(id),
                ));
            }
        }
        RoomKey::Collection(id) if state.is_live(id.into()) => {
            entries.push(item(
                "room-rename",
                Icon::Pencil,
                "Rename collection",
                MenuAction::RenameCollection(id),
            ));
            entries.extend(colour.map(|hue| colour_item("room-colour", id.into(), hue)));
        }
        _ => {}
    }
    entries.push(completed_item(show_completed));
    match room {
        RoomKey::List(id) if state.is_live(id.into()) => {
            entries.push(MenuEntry::Separator);
            entries.push(danger(
                "room-delete",
                Icon::TrashTwo,
                "Delete list…",
                MenuAction::AskDeleteList(id),
            ));
        }
        RoomKey::Collection(id) if state.is_live(id.into()) => {
            entries.push(MenuEntry::Separator);
            entries.push(danger(
                "room-delete",
                Icon::TrashTwo,
                "Delete collection…",
                MenuAction::AskDeleteCollection(id),
            ));
        }
        _ => {}
    }
    entries
}

/// A smart list's room menu: the Completed toggle, where it has a Completed section.
/// The Activity room's menu (the header's "more" button): Refresh activity, then whose changes it
/// shows: every change, or one of the agents the sidebar lists, the current choice ticked.
pub fn activity_menu(agents: &[AgentRow], current: Option<AgentKey>) -> Vec<MenuEntry> {
    let mut entries = vec![
        item(
            "activity-refresh",
            Icon::RefreshCw,
            todo_client::activity::REFRESH,
            MenuAction::RefreshActivity,
        ),
        MenuEntry::Separator,
        with(
            item(
                "activity-all",
                Icon::Clock,
                "All changes",
                MenuAction::FilterActivity(None),
            ),
            |i| MenuItem {
                checked: current.is_none(),
                ..i
            },
        ),
    ];
    // The agent the feed is filtered to stays offered even once the sidebar no longer lists it.
    let mut keys: Vec<(AgentKey, SharedString, Option<Hue>)> = agents
        .iter()
        .map(|a| (a.key, a.name.clone(), Some(a.agent.hue())))
        .collect();
    if let Some(key) = current.filter(|k| !keys.iter().any(|(a, ..)| a == k)) {
        keys.push((key, key.name().into(), None));
    }
    entries.extend(keys.into_iter().map(|(key, name, hue)| {
        with(
            item(
                format!("activity-agent-{}", key.as_str()),
                Icon::Sparkles,
                format!("Only {name}"),
                MenuAction::FilterActivity(Some(key)),
            ),
            |i| MenuItem {
                dot: hue,
                checked: current == Some(key),
                ..i
            },
        )
    }));
    entries
}

fn completed_only(room: RoomKey, show_completed: bool) -> Vec<MenuEntry> {
    if has_completed_section(room) {
        vec![completed_item(show_completed)]
    } else {
        Vec::new()
    }
}

/// A list's context menu in the sidebar, or (`page`) its delete confirmation, collection picker or
/// colour swatches.
pub fn list_menu(state: &State, id: ListId, page: Page) -> Vec<MenuEntry> {
    if !state.is_live(id.into()) {
        return Vec::new();
    }
    match page {
        Page::Confirm => {
            return delete_list_confirm(state, id, ("list-delete-confirm", "list-delete-cancel"))
                .into_iter()
                .collect();
        }
        Page::Move => return move_list_menu(state, id),
        Page::Colour => return colour_menu(state, id.into()),
        Page::Main => {}
    }
    let mut entries = vec![item(
        "list-rename",
        Icon::Pencil,
        "Rename",
        MenuAction::RenameList(id),
    )];
    entries
        .extend(colour_of(state, id.into()).map(|hue| colour_item("list-colour", id.into(), hue)));
    if commands::can_move_list(state, id) {
        entries.push(item(
            "list-move",
            Icon::List,
            "Move to…",
            MenuAction::AskMoveList(id),
        ));
    }
    entries.extend([
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
    ]);
    entries
}

/// The collection picker that moves a list (its menu's Move to…): every collection, as the sidebar
/// orders them, with its glyph and colour, then "No collection" for the top level. Where the list is
/// now is ticked. Empty once the list is gone.
pub fn move_list_menu(state: &State, id: ListId) -> Vec<MenuEntry> {
    let Some(list) = state.lists.get(&id).filter(|_| state.is_live(id.into())) else {
        return Vec::new();
    };
    let collections: Vec<MenuEntry> = commands::collections(state)
        .into_iter()
        .filter_map(|c| state.collections.get(&c))
        .map(|c| {
            MenuEntry::Item(MenuItem {
                key: format!("move-list-{}", c.id).into(),
                icon: Icon::List,
                glyph: Some((
                    lists::list_glyph(&c.name),
                    lists::hue(c.color.as_deref(), c.id.as_uuid().as_bytes()),
                )),
                dot: None,
                label: labels::one_line(&c.name).into(),
                danger: false,
                checked: list.collection == Some(c.id),
                hint: None,
                action: MenuAction::MoveList(id, Some(c.id)),
                more: None,
            })
        })
        .collect();
    let mut entries = Vec::new();
    if !collections.is_empty() {
        entries.push(MenuEntry::Heading("Collections".into()));
        entries.extend(collections);
        entries.push(MenuEntry::Separator);
    }
    entries.push(with(
        item(
            "move-list-none",
            Icon::List,
            "No collection",
            MenuAction::MoveList(id, None),
        ),
        |i| MenuItem {
            checked: list.collection.is_none(),
            ..i
        },
    ));
    entries
}

/// A collection's context menu in the sidebar, or (`page`) its delete confirmation or colour swatches.
pub fn collection_menu(
    state: &State,
    id: CollectionId,
    collapsed: bool,
    page: Page,
) -> Vec<MenuEntry> {
    if !state.is_live(id.into()) {
        return Vec::new();
    }
    match page {
        Page::Confirm => {
            return delete_collection_confirm(
                state,
                id,
                ("collection-delete-confirm", "collection-delete-cancel"),
            )
            .into_iter()
            .collect();
        }
        Page::Colour => return colour_menu(state, id.into()),
        Page::Move | Page::Main => {}
    }
    let (icon, label) = if collapsed {
        (Icon::ChevronDown, "Expand")
    } else {
        (Icon::ChevronRight, "Collapse")
    };
    let mut entries = vec![
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
    ];
    entries.extend(
        colour_of(state, id.into()).map(|hue| colour_item("collection-colour", id.into(), hue)),
    );
    entries.extend([
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
        MenuEntry::Separator,
        danger(
            "collection-delete",
            Icon::TrashTwo,
            "Delete collection…",
            MenuAction::AskDeleteCollection(id),
        ),
    ]);
    entries
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
        entries.push(item(
            "device-settings",
            Icon::Link,
            "Devices…",
            MenuAction::DeviceSettings,
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
            dot: None,
            label: label.into(),
            danger: false,
            checked: level == current,
            hint: (!marks.is_empty()).then(|| marks.into()),
            action: MenuAction::SetPriority(task, level),
            more: None,
        })
    })
    .collect()
}

/// A task's context menu (right-click a row): what its row and the list's keys already do (complete or
/// reopen, My Day, Important, the priority picker), then Move to…, Links… and Delete. Deleting doesn't
/// ask: Undo brings the task back. Empty once the task is gone.
pub fn task_menu(state: &State, task: TaskId, today: Date) -> Vec<MenuEntry> {
    let Some(t) = commands::organize::actionable_task(state, task) else {
        return Vec::new();
    };
    let done = if t.is_done() { "Reopen" } else { "Complete" };
    let (sun, my_day) = if t.my_day == Some(today) {
        (Icon::SunFilled, "Remove from My Day")
    } else {
        (Icon::Sun, "Add to My Day")
    };
    let (star, important) = if t.priority == Priority::High {
        (Icon::StarFilled, "Remove importance")
    } else {
        (Icon::Star, "Mark as important")
    };
    let marks = priority_label(t.priority).1;
    let priority = with(
        item(
            "task-priority",
            Icon::Flag,
            "Priority…",
            MenuAction::AskPriority(task),
        ),
        |i| MenuItem {
            hint: (!marks.is_empty()).then(|| marks.into()),
            ..i
        },
    );
    let links = live_links(state, task).len();
    let links = with(
        item(
            "task-links",
            Icon::Link,
            "Links…",
            MenuAction::AskLinks(task),
        ),
        |i| MenuItem {
            hint: (links > 0).then(|| links.to_string().into()),
            ..i
        },
    );
    vec![
        item(
            "task-done",
            Icon::CircleCheck,
            done,
            MenuAction::ToggleDone(task),
        ),
        item("task-my-day", sun, my_day, MenuAction::ToggleMyDay(task)),
        item(
            "task-important",
            star,
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
        item(
            "task-merge",
            Icon::ListPlus,
            "Merge into…",
            MenuAction::PickMerge(task),
        ),
        links,
        MenuEntry::Separator,
        danger(
            "task-delete",
            Icon::TrashTwo,
            "Delete task",
            MenuAction::DeleteTask(task),
        ),
    ]
}

/// The list picker that moves a task (Move to…): the lists grouped as the sidebar shows them, each
/// collection's under its name, the task's own list ticked. What's typed (`filter`) narrows them by
/// name or collection. Empty once the task is gone.
pub fn move_menu(state: &State, task: TaskId, filter: &str) -> Vec<MenuEntry> {
    let Some(groups) = todo_client::intent::organize::task_move_groups(state, task, filter) else {
        return Vec::new();
    };
    let mut entries = vec![MenuEntry::Filter {
        typed: filter.to_string().into(),
        hint: "Type to filter lists",
    }];
    let found = !groups.is_empty();
    for group in groups {
        entries.extend(group.heading.map(|h| MenuEntry::Heading(h.into())));
        entries.extend(group.lists.into_iter().map(|dest| {
            let l = dest.list;
            MenuEntry::Item(MenuItem {
                key: format!("move-{}", l.id).into(),
                icon: Icon::List,
                glyph: Some((
                    lists::list_glyph(&l.name),
                    lists::hue(l.color.as_deref(), l.id.as_uuid().as_bytes()),
                )),
                dot: None,
                label: labels::one_line(&l.name).into(),
                danger: false,
                checked: dest.current,
                hint: None,
                action: MenuAction::MoveTask(task, l.id),
                more: None,
            })
        }));
    }
    if !found {
        let typed = labels::one_line(filter.trim());
        entries.push(MenuEntry::Note(
            format!("No list matches \u{201c}{typed}\u{201d}.").into(),
        ));
    }
    entries
}

pub fn merge_picker(state: &State, source: TaskId, filter: &str) -> Vec<MenuEntry> {
    let Some(candidates) = merge::candidates(state, source, filter) else {
        return Vec::new();
    };
    let mut entries = vec![MenuEntry::Filter {
        typed: filter.to_string().into(),
        hint: "Type to filter open tasks",
    }];
    if candidates.is_empty() {
        entries.push(MenuEntry::Note(
            if filter.trim().is_empty() {
                merge::EMPTY
            } else {
                merge::NO_MATCHES
            }
            .into(),
        ));
    }
    entries.extend(candidates.into_iter().map(|c| {
        with(
            item(
                format!("merge-{}", c.id),
                Icon::ChevronRight,
                c.title,
                MenuAction::AskMerge(source, c.id),
            ),
            |i| MenuItem {
                hint: Some(c.location.into()),
                ..i
            },
        )
    }));
    entries
}

pub fn merge_confirmation(state: &State, source: TaskId, target: TaskId) -> Vec<MenuEntry> {
    merge::preview(state, source, target).map_or_else(Vec::new, |preview| {
        vec![confirm(
            preview.question,
            item(
                "merge-confirm",
                Icon::ListPlus,
                "Merge tasks",
                MenuAction::Merge(source, target),
            ),
            "merge-cancel",
        )]
    })
}

/// A task's live links, oldest first, as the detail pane lists them.
fn live_links(state: &State, task: TaskId) -> Vec<&todo_core::model::Link> {
    let mut links: Vec<_> = state
        .links
        .values()
        .filter(|l| l.task == task && state.is_live(l.id.into()))
        .collect();
    links.sort_by_key(|l| (l.created.at, l.id));
    links
}

/// What a link is called: its title, else its address (or an email's subject).
pub fn link_name(kind: &todo_core::model::LinkKind) -> String {
    use todo_core::model::LinkKind;
    match kind {
        LinkKind::Web { url, title } => title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(url)
            .to_string(),
        LinkKind::Email(e) => e.subject.clone(),
    }
}

/// A task's links (its menu's Links…): add one (in the detail pane's link field), or remove one, without
/// asking: Undo brings it back. Empty once the task is gone.
pub fn links_menu(state: &State, task: TaskId) -> Vec<MenuEntry> {
    if !state.is_live(task.into()) {
        return Vec::new();
    }
    let mut entries = vec![
        MenuEntry::Heading("Links".into()),
        item(
            "link-add",
            Icon::Plus,
            "Add a link…",
            MenuAction::AddLink(task),
        ),
    ];
    let links = live_links(state, task);
    if !links.is_empty() {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Heading("Remove".into()));
    }
    entries.extend(links.into_iter().map(|l| {
        danger(
            format!("link-remove-{}", l.id),
            Icon::TrashTwo,
            labels::one_line(&link_name(&l.kind)),
            MenuAction::RemoveLink(l.id),
        )
    }));
    entries
}

/// A tag's colour as drawn: its swatch, or one picked from its ID.
pub fn tag_hue(tag: &todo_core::model::Tag) -> Hue {
    lists::hue(tag.color.as_deref(), tag.id.as_uuid().as_bytes())
}

/// The tag picker (the detail pane's Tags row, or T): every tag, by name, those on the task ticked;
/// choosing one puts it on or takes it off, and the picker stays open. What's typed (`filter`) narrows
/// them, and when no tag has exactly that name, the first item creates it. Each tag's chevron (or Right)
/// opens its own page. Empty once the task is gone.
pub fn tag_menu(state: &State, task: TaskId, filter: &str) -> Vec<MenuEntry> {
    let Some(t) = state
        .tasks
        .get(&task)
        .filter(|_| state.is_live(task.into()))
    else {
        return Vec::new();
    };
    let mut entries = vec![MenuEntry::Filter {
        typed: filter.to_string().into(),
        hint: "Type to find or create a tag",
    }];
    let typed = labels::one_line(filter.trim());
    if !typed.is_empty() && commands::tag_named(state, &typed).is_none() {
        entries.push(item(
            "tag-create",
            Icon::Plus,
            format!("Create tag \u{201c}{typed}\u{201d}"),
            MenuAction::CreateTag(task),
        ));
    }
    let words = fold(filter);
    let tags: Vec<MenuEntry> = commands::tags(state)
        .into_iter()
        .filter(|tag| {
            let name = fold(&tag.name);
            words.split_whitespace().all(|w| name.contains(w))
        })
        .map(|tag| {
            MenuEntry::Item(MenuItem {
                key: format!("tag-{}", tag.id).into(),
                icon: Icon::Hash,
                glyph: None,
                dot: Some(tag_hue(tag)),
                label: labels::one_line(&tag.name).into(),
                danger: false,
                checked: t.tags.contains(&tag.id),
                hint: None,
                action: MenuAction::ToggleTag(task, tag.id),
                more: Some(MenuAction::EditTag(task, tag.id)),
            })
        })
        .collect();
    if tags.is_empty() && typed.is_empty() {
        entries.push(MenuEntry::Note(
            "No tags yet. Type a name to create one.".into(),
        ));
    }
    entries.extend(tags);
    entries
}

/// A tag's own page in the tag picker: put it on the task or take it off, rename it, recolour it, or go
/// back to every tag. Empty once the tag or the task is gone.
pub fn tag_edit_menu(state: &State, task: TaskId, tag: TagId) -> Vec<MenuEntry> {
    let (Some(t), Some(g)) = (
        state
            .tasks
            .get(&task)
            .filter(|_| state.is_live(task.into())),
        state.tags.get(&tag).filter(|_| state.is_live(tag.into())),
    ) else {
        return Vec::new();
    };
    let on = t.tags.contains(&tag);
    vec![
        MenuEntry::Heading(format!("#{}", labels::one_line(&g.name)).into()),
        item(
            "tag-toggle",
            if on { Icon::X } else { Icon::Hash },
            if on {
                "Remove from task"
            } else {
                "Add to task"
            },
            MenuAction::ToggleTag(task, tag),
        ),
        item(
            "tag-rename",
            Icon::Pencil,
            "Rename tag",
            MenuAction::RenameTag(tag),
        ),
        colour_item("tag-colour", tag.into(), tag_hue(g)),
        danger(
            "tag-ask-delete",
            Icon::TrashTwo,
            "Delete tag…",
            MenuAction::AskDeleteTag(tag),
        ),
        MenuEntry::Separator,
        item(
            "tag-back",
            Icon::ChevronLeft,
            "All tags",
            MenuAction::AllTags(task),
        ),
    ]
}

/// Tombstoning a tag hides it on every task, but does not delete tasks or their memberships.
pub fn tag_confirm(state: &State, task: TaskId, tag: TagId) -> Vec<MenuEntry> {
    if !state.is_live(task.into()) {
        return vec![];
    }
    let Some(question) = todo_client::intent::confirm::tag_question(state, tag) else {
        return vec![];
    };
    vec![confirm(
        question,
        danger(
            "tag-delete",
            Icon::TrashTwo,
            "Delete tag",
            MenuAction::DeleteTag(tag),
        ),
        "tag-cancel",
    )]
}

/// The refused changes the open "couldn't sync" list shows: of those there now (`failed`), the ones
/// that were there when it opened (`shown`). One dismissed since is gone; one refused since waits for
/// the list to open again, so the rows never shift under the pointer and Dismiss all takes only what
/// was seen.
pub fn shown_failures(failed: Vec<FailedEntry>, shown: &[CommandId]) -> Vec<FailedEntry> {
    failed
        .into_iter()
        .filter(|f| shown.contains(&f.entry.command_id))
        .collect()
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
                dot: None,
                label: "Dismiss".into(),
                danger: false,
                checked: false,
                hint: None,
                action: MenuAction::DismissFailure(id),
                more: None,
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
    todo_client::native::admin_url(server, todo_client::native::Cleartext::Loopback)
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
    fn a_lists_room_menu_renames_recolours_moves_toggles_completed_and_deletes() {
        let f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        let menu = room_menu(&f.state, RoomKey::List(office), false, Page::Main);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::RenameList(office),
                MenuAction::AskColour(office.into()),
                MenuAction::AskMoveList(office),
                MenuAction::ToggleCompleted,
                MenuAction::AskDeleteList(office),
            ]
        );
        // Colour… shows the list's own colour.
        let colour = items(&menu).nth(1).expect("colour");
        let list = &f.state.lists[&office];
        assert_eq!(
            colour.dot,
            Some(lists::hue(
                list.color.as_deref(),
                office.as_uuid().as_bytes()
            ))
        );
        assert!(items(&menu).last().is_some_and(|i| i.danger));
        // Confirming says, in full, what goes with it, above short buttons: Delete list and Cancel.
        let confirm = room_menu(&f.state, RoomKey::List(office), false, Page::Confirm);
        assert_eq!(
            actions(&confirm),
            [MenuAction::DeleteList(office), MenuAction::Cancel]
        );
        assert!(is_confirmation(&confirm));
        let [
            MenuEntry::Confirm {
                message, buttons, ..
            },
        ] = confirm.as_slice()
        else {
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
        let menu = list_menu(&f.state, office, Page::Confirm);
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
        let menu = list_menu(&f.state, office, Page::Confirm);
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
        let tag = TagId::from_uuid(uuid::Uuid::nil());
        for action in [
            MenuAction::RenameList(list),
            MenuAction::RenameCollection(col),
            MenuAction::NewListIn(col),
            MenuAction::DeleteList(list),
            MenuAction::DeleteCollection(col),
            MenuAction::RenameTag(tag),
            MenuAction::AddLink(task),
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
            MenuAction::AskDeleteCollection(col),
            MenuAction::AskMoveList(list),
            MenuAction::MoveList(list, Some(col)),
            MenuAction::AskColour(list.into()),
            MenuAction::SetColour(col.into(), "rose"),
            MenuAction::ToggleTag(task, tag),
            MenuAction::CreateTag(task),
            MenuAction::EditTag(task, tag),
            MenuAction::AllTags(task),
            MenuAction::AskLinks(task),
            MenuAction::RemoveLink(LinkId::from_uuid(uuid::Uuid::nil())),
        ] {
            assert!(!action.moves_focus(), "{action:?}");
        }
    }

    #[test]
    fn smart_rooms_offer_only_what_applies() {
        let f = fixtures::my_day();
        for smart in Smart::ALL {
            let menu = room_menu(&f.state, RoomKey::Smart(smart), false, Page::Main);
            let expected: &[MenuAction] = match smart {
                Smart::Completed | Smart::AgentActivity => &[],
                _ => &[MenuAction::ToggleCompleted],
            };
            assert_eq!(actions(&menu), expected, "{smart:?}");
            // A smart list can't be deleted, even when asked to confirm.
            assert!(
                !actions(&room_menu(
                    &f.state,
                    RoomKey::Smart(smart),
                    false,
                    Page::Confirm
                ))
                .iter()
                .any(|a| matches!(a, MenuAction::DeleteList(_)))
            );
        }
    }

    #[test]
    fn a_collections_room_menu_renames_recolours_and_deletes_it() {
        let f = fixtures::work();
        let (&id, _) = f.state.collections.iter().next().expect("collection");
        let menu = room_menu(&f.state, RoomKey::Collection(id), true, Page::Main);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::RenameCollection(id),
                MenuAction::AskColour(id.into()),
                MenuAction::ToggleCompleted,
                MenuAction::AskDeleteCollection(id),
            ]
        );
        assert_eq!(items(&menu).nth(2).expect("toggle").label, "Hide completed");
        assert!(items(&menu).last().is_some_and(|i| i.danger));
        // A collection's own pages: its confirmation and its colours. It has no collection picker.
        let confirm = room_menu(&f.state, RoomKey::Collection(id), true, Page::Confirm);
        assert_eq!(
            actions(&confirm),
            [MenuAction::DeleteCollection(id), MenuAction::Cancel]
        );
        assert!(is_grid(&room_menu(
            &f.state,
            RoomKey::Collection(id),
            true,
            Page::Colour
        )));
        assert!(room_menu(&f.state, RoomKey::Collection(id), true, Page::Move).is_empty());
    }

    #[test]
    fn a_collections_menu_offers_colour_and_delete_and_its_confirmation_counts_what_goes() {
        let f = fixtures::my_day();
        let work = f
            .state
            .collections
            .values()
            .find(|c| c.name == "Work")
            .expect("Work")
            .id;
        let menu = collection_menu(&f.state, work, false, Page::Main);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::ToggleCollection(work),
                MenuAction::NewListIn(work),
                MenuAction::RenameCollection(work),
                MenuAction::AskColour(work.into()),
                MenuAction::Nudge(work.into(), -1),
                MenuAction::Nudge(work.into(), 1),
                MenuAction::AskDeleteCollection(work),
            ]
        );
        assert!(matches!(menu[menu.len() - 2], MenuEntry::Separator));
        let confirm = collection_menu(&f.state, work, false, Page::Confirm);
        let [
            MenuEntry::Confirm {
                message, buttons, ..
            },
        ] = confirm.as_slice()
        else {
            panic!("{confirm:?}");
        };
        let contents = commands::collection_contents(&f.state, work);
        assert_eq!(contents.lists, 4);
        assert_eq!(
            message.as_ref(),
            format!(
                "Delete the collection “Work” with its 4 lists and their {} tasks? Undo brings \
                 them all back.",
                contents.tasks
            )
        );
        assert_eq!(buttons[0].label, "Delete collection");
        assert!(buttons[0].danger);
        assert_eq!(first_highlight(&confirm), Some(1), "on Cancel");

        // The wording follows what goes with it.
        let mut state = f.state.clone();
        let say =
            |state: &State| match collection_menu(state, work, false, Page::Confirm).as_slice() {
                [MenuEntry::Confirm { message, .. }] => message.to_string(),
                other => panic!("{other:?}"),
            };
        let lists: Vec<ListId> = state
            .lists
            .values()
            .filter(|l| l.collection == Some(work))
            .map(|l| l.id)
            .collect();
        let gone = |state: &mut State, keep: usize| {
            let tomb = todo_core::model::Tombstone {
                at: f.now.at,
                by: todo_core::Actor::System,
            };
            for l in &lists[keep..] {
                state.lists.get_mut(l).expect("list").deleted = Some(tomb.clone());
            }
        };
        gone(&mut state, 1);
        assert!(
            say(&state).contains("with its list and its "),
            "{}",
            say(&state)
        );
        for t in state.tasks.values_mut().filter(|t| t.list == lists[0]) {
            t.deleted = Some(todo_core::model::Tombstone {
                at: f.now.at,
                by: todo_core::Actor::System,
            });
        }
        assert_eq!(
            say(&state),
            "Delete the collection “Work” with its list? Undo brings them all back."
        );
        gone(&mut state, 0);
        assert_eq!(
            say(&state),
            "Delete the collection “Work”? Undo brings it back."
        );
    }

    #[test]
    fn a_delete_confirmed_after_a_sync_changed_it_asks_again_and_deletes_nothing() {
        use crate::store::Store;
        use todo_core::Command;
        use todo_core::command::NewTask;
        let f = fixtures::my_day();
        let store = Store::fixture(&f);
        let state = store.view();
        let work = commands::collections(&state)[0];
        let office = list_named(&state, "Office");
        // Drawn: the question as asked.
        let asked = Asked::of(&collection_menu(&state, work, false, Page::Confirm)).expect("asked");
        assert!(!asked.changed);
        // A sync between drawing it and pressing Delete: a task lands in one of its lists.
        let task = todo_core::ids::TaskId::from_uuid(store.new_uuid());
        store
            .submit(Command::CreateTask(Box::new(NewTask::new(
                task, office, "Synced",
            ))))
            .expect("synced");
        let now = collection_menu(&store.view(), work, false, Page::Confirm);
        // Until confirmed, it still shows what was asked, not a question redrawn under the pointer.
        let shown = as_asked(now.clone(), &asked);
        assert_eq!(question(&shown), Some(&asked.question));
        assert!(matches!(&shown[0], MenuEntry::Confirm { note: None, .. }));
        // Confirming deletes nothing: the new question is asked, saying why.
        let Confirmed::AskAgain(again) = on_confirm(Some(&asked), &now) else {
            panic!("expected to ask again");
        };
        assert_ne!(again, asked.question);
        assert_eq!(Some(&again), question(&now));
        let asked_again = Asked {
            question: again,
            changed: true,
        };
        let shown = as_asked(now.clone(), &asked_again);
        let [
            MenuEntry::Confirm {
                note: Some(note),
                buttons,
                ..
            },
        ] = shown.as_slice()
        else {
            panic!("{shown:?}");
        };
        assert_eq!(
            note.as_ref(),
            "The collection changed while the question was open. Check it, then confirm again."
        );
        assert_eq!(buttons[0].action, MenuAction::DeleteCollection(work));
        assert_eq!(first_highlight(&shown), Some(1), "on Cancel again");
        // Confirmed again with nothing new, it goes ahead.
        assert_eq!(on_confirm(Some(&asked_again), &now), Confirmed::Proceed);
        // A list's question works the same way, with its own words; a rename counts too.
        let list_asked =
            Asked::of(&list_menu(&store.view(), office, Page::Confirm)).expect("asked");
        store
            .submit(commands::rename_list(&store.view(), office, "Desk").expect("rename"))
            .expect("renamed elsewhere");
        let list_now = list_menu(&store.view(), office, Page::Confirm);
        let Confirmed::AskAgain(q) = on_confirm(Some(&list_asked), &list_now) else {
            panic!("expected to ask again");
        };
        let shown = as_asked(
            list_now,
            &Asked {
                question: q,
                changed: true,
            },
        );
        assert!(
            matches!(&shown[0], MenuEntry::Confirm { note: Some(n), .. } if n.starts_with("The list changed"))
        );
        // Gone meanwhile: nothing to confirm.
        store
            .submit(commands::delete_collection(&store.view(), work).expect("delete"))
            .expect("deleted elsewhere");
        assert_eq!(
            on_confirm(
                Some(&asked_again),
                &collection_menu(&store.view(), work, false, Page::Confirm)
            ),
            Confirmed::Gone
        );
    }

    #[test]
    fn the_collection_picker_ticks_where_a_list_is_and_offers_the_top_level() {
        let f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        let menu = move_list_menu(&f.state, office);
        let collections = commands::collections(&f.state);
        let mut expected: Vec<MenuAction> = collections
            .iter()
            .map(|&c| MenuAction::MoveList(office, Some(c)))
            .collect();
        expected.push(MenuAction::MoveList(office, None));
        assert_eq!(actions(&menu), expected);
        let ticked: Vec<MenuAction> = items(&menu)
            .filter(|i| i.checked)
            .map(|i| i.action)
            .collect();
        let own = f.state.lists[&office].collection;
        assert_eq!(ticked, [MenuAction::MoveList(office, own)]);
        // The keyboard starts where the list is; the collections wear their glyphs and colours.
        assert_eq!(
            first_highlight(&menu)
                .and_then(|i| items(&menu).nth(i))
                .map(|i| i.action),
            Some(MenuAction::MoveList(office, own))
        );
        assert!(
            items(&menu)
                .take(collections.len())
                .all(|i| i.glyph.is_some())
        );
        assert!(matches!(&menu[0], MenuEntry::Heading(h) if h == "Collections"));
        // A top-level list has "No collection" ticked; the list and room menus open this page.
        let mut top = f.state.clone();
        top.lists.get_mut(&office).expect("list").collection = None;
        let ticked: Vec<MenuAction> = items(&move_list_menu(&top, office))
            .filter(|i| i.checked)
            .map(|i| i.action)
            .collect();
        assert_eq!(ticked, [MenuAction::MoveList(office, None)]);
        assert_eq!(list_menu(&f.state, office, Page::Move), menu);
        assert_eq!(
            room_menu(&f.state, RoomKey::List(office), false, Page::Move),
            menu
        );
        // With nowhere to go (a top-level list and no collections), no Move to….
        let mut alone = top.clone();
        alone.collections.clear();
        assert!(
            !actions(&list_menu(&alone, office, Page::Main))
                .contains(&MenuAction::AskMoveList(office))
        );
    }

    #[test]
    fn the_colour_page_offers_every_swatch_and_ticks_the_current_one() {
        let f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        let menu = colour_menu(&f.state, office.into());
        assert!(matches!(&menu[0], MenuEntry::Heading(h) if h == "List colour"));
        assert!(is_grid(&menu));
        let swatches: Vec<&MenuItem> = items(&menu).collect();
        assert_eq!(swatches.len(), hues::SWATCHES.len());
        for (item, swatch) in swatches.iter().zip(hues::SWATCHES) {
            assert_eq!(
                item.action,
                MenuAction::SetColour(office.into(), swatch.key)
            );
            assert_eq!(item.dot, Some(swatch.degrees));
            assert!(!item.label.is_empty());
        }
        // Office is cornflower in the sample data.
        let ticked: Vec<&str> = swatches
            .iter()
            .filter(|i| i.checked)
            .map(|i| i.label.as_ref())
            .collect();
        assert_eq!(ticked, ["Cornflower"]);
        // The keyboard starts on it.
        assert_eq!(
            first_highlight(&menu).map(|i| swatches[i].label.as_ref()),
            Some("Cornflower")
        );
        // A collection's and a tag's pages say whose colour it is; a task has none.
        let work = commands::collections(&f.state)[0];
        assert!(
            matches!(&colour_menu(&f.state, work.into())[0], MenuEntry::Heading(h) if h == "Collection colour")
        );
        let tag = commands::tags(&f.state)[0].id;
        assert!(
            matches!(&colour_menu(&f.state, tag.into())[0], MenuEntry::Heading(h) if h == "Tag colour")
        );
        let task = f.ui.task.expect("task");
        assert!(colour_menu(&f.state, task.into()).is_empty());
    }

    #[test]
    fn the_arrows_walk_the_swatches_as_a_grid() {
        let n = hues::SWATCHES.len();
        assert_eq!(grid_step(None, "down", n), Some(0));
        assert_eq!(grid_step(None, "right", n), Some(0));
        assert_eq!(grid_step(None, "up", n), Some(n - 1));
        assert_eq!(grid_step(None, "left", n), Some(n - 1));
        // Left and Right go through them in order, wrapping round.
        assert_eq!(grid_step(Some(0), "right", n), Some(1));
        assert_eq!(grid_step(Some(n - 1), "right", n), Some(0));
        assert_eq!(grid_step(Some(0), "left", n), Some(n - 1));
        // Up and Down move a row and stay put at the edges.
        assert_eq!(grid_step(Some(1), "down", n), Some(1 + SWATCH_COLUMNS));
        assert_eq!(grid_step(Some(1 + SWATCH_COLUMNS), "up", n), Some(1));
        assert_eq!(grid_step(Some(1), "up", n), Some(1));
        assert_eq!(grid_step(Some(n - 1), "down", n), Some(n - 1));
        assert_eq!(grid_step(None, "down", 0), None);
    }

    #[test]
    fn the_tag_picker_ticks_the_tasks_tags_finds_by_name_and_offers_to_create() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let menu = tag_menu(&f.state, task, "");
        assert!(matches!(&menu[0], MenuEntry::Filter { typed, .. } if typed.is_empty()));
        let all = commands::tags(&f.state);
        assert_eq!(items(&menu).count(), all.len(), "every tag, once");
        // By name, each with its colour, toggling it on the task and opening its own page on the side.
        for (item, tag) in items(&menu).zip(&all) {
            assert_eq!(item.label, labels::one_line(&tag.name));
            assert_eq!(item.action, MenuAction::ToggleTag(task, tag.id));
            assert_eq!(item.more, Some(MenuAction::EditTag(task, tag.id)));
            assert_eq!(item.dot, Some(tag_hue(tag)));
        }
        let ticked: Vec<&str> = items(&menu)
            .filter(|i| i.checked)
            .map(|i| i.label.as_ref())
            .collect();
        assert_eq!(ticked, ["planning", "q4"]);
        // Typing narrows them, ignoring case and accents; a tag's own name (in any case) offers no
        // Create, while the same letters without the accent are another name, which can be created.
        let narrowed = tag_menu(&f.state, task, "DOMÁCNOST");
        let labels: Vec<&str> = items(&narrowed).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["domácnost"]);
        let plain = tag_menu(&f.state, task, "domacnost");
        let labels: Vec<&str> = items(&plain).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Create tag “domacnost”", "domácnost"]);
        let exact = tag_menu(&f.state, task, "docs");
        assert!(!actions(&exact).contains(&MenuAction::CreateTag(task)));
        // A new name: Create first, so Enter makes it at once.
        let new = tag_menu(&f.state, task, "receipts");
        assert_eq!(actions(&new), [MenuAction::CreateTag(task)]);
        assert_eq!(
            items(&new).next().map(|i| i.label.as_ref()),
            Some("Create tag “receipts”")
        );
        assert_eq!(first_highlight(&new), Some(0));
        // No tags at all: a quiet line says how to make one.
        let mut bare = f.state.clone();
        bare.tags.clear();
        let empty = tag_menu(&bare, task, "");
        assert!(matches!(empty.last(), Some(MenuEntry::Note(_))));
        // Gone, it offers nothing.
        let mut gone = f.state.clone();
        gone.tasks.get_mut(&task).expect("task").deleted = Some(todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        });
        assert!(tag_menu(&gone, task, "").is_empty());
    }

    #[test]
    fn a_tags_own_page_toggles_renames_recolours_and_goes_back() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let planning = commands::tag_named(&f.state, "planning").expect("tag");
        let docs = commands::tag_named(&f.state, "docs").expect("tag");
        let menu = tag_edit_menu(&f.state, task, planning);
        assert!(matches!(&menu[0], MenuEntry::Heading(h) if h == "#planning"));
        assert_eq!(
            actions(&menu),
            [
                MenuAction::ToggleTag(task, planning),
                MenuAction::RenameTag(planning),
                MenuAction::AskColour(planning.into()),
                MenuAction::AskDeleteTag(planning),
                MenuAction::AllTags(task),
            ]
        );
        let label = |m: &[MenuEntry]| items(m).next().map(|i| i.label.to_string());
        assert_eq!(label(&menu).as_deref(), Some("Remove from task"));
        assert_eq!(
            label(&tag_edit_menu(&f.state, task, docs)).as_deref(),
            Some("Add to task")
        );
        let mut gone = f.state.clone();
        gone.tags.get_mut(&planning).expect("tag").deleted = Some(todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        });
        assert!(tag_edit_menu(&gone, task, planning).is_empty());
    }

    #[test]
    fn a_tags_count_excludes_deleted_tasks_and_tasks_in_a_deleted_list() {
        let mut f = fixtures::my_day();
        let task = f.ui.task.expect("open task");
        let tag = commands::tag_named(&f.state, "planning").expect("tag");
        let list = f.state.tasks[&task].list;
        let other = f
            .state
            .tasks
            .values()
            .find(|t| t.list != list)
            .expect("other list")
            .list;
        let tomb = todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        };
        f.state.lists.get_mut(&other).expect("list").deleted = Some(tomb.clone());
        for t in f.state.tasks.values_mut() {
            t.tags.insert(tag);
            if t.id != task && t.list != other {
                t.deleted = Some(tomb.clone());
            }
        }
        let menu = tag_confirm(&f.state, task, tag);
        assert!(question(&menu).expect("question").contains("on 1 task."));
        f.state
            .tasks
            .get_mut(&task)
            .expect("open task")
            .tags
            .remove(&tag);
        let menu = tag_confirm(&f.state, task, tag);
        assert_eq!(
            question(&menu).expect("question").as_ref(),
            "Delete the tag “planning”? Undo restores the tag."
        );
    }

    #[test]
    fn deleting_a_tag_rechecks_its_live_task_count_and_name() {
        let mut f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let tag = commands::tag_named(&f.state, "planning").expect("tag");
        let menu = tag_confirm(&f.state, task, tag);
        let asked = Asked::of(&menu).expect("confirmation");
        assert_eq!(
            chosen(
                &menu,
                key_at(&menu, first_highlight(&menu).expect("cancel")).as_deref()
            ),
            Some(MenuAction::Cancel)
        );
        assert_eq!(on_confirm(Some(&asked), &menu), Confirmed::Proceed);
        f.state
            .tasks
            .get_mut(&task)
            .expect("task")
            .tags
            .remove(&tag);
        assert!(matches!(
            on_confirm(Some(&asked), &tag_confirm(&f.state, task, tag)),
            Confirmed::AskAgain(_)
        ));
        f.state.tags.remove(&tag);
        assert_eq!(
            on_confirm(Some(&asked), &tag_confirm(&f.state, task, tag)),
            Confirmed::Gone
        );
    }

    #[test]
    fn a_tasks_links_page_adds_one_or_removes_each() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        let link = *f
            .state
            .links
            .values()
            .find(|l| l.task == task)
            .map(|l| &l.id)
            .expect("a link");
        let menu = links_menu(&f.state, task);
        assert_eq!(
            actions(&menu),
            [MenuAction::AddLink(task), MenuAction::RemoveLink(link)]
        );
        let remove = items(&menu).nth(1).expect("remove");
        assert!(remove.danger);
        assert_eq!(remove.label, "Q4 roadmap: pre-read and agenda");
        // The task's menu says how many it has.
        let task_menu = task_menu(&f.state, task, f.now.today());
        let links = items(&task_menu)
            .find(|i| i.action == MenuAction::AskLinks(task))
            .expect("Links…");
        assert_eq!(links.hint.as_deref(), Some("1"));
        // Without links: only Add.
        let mut bare = f.state.clone();
        bare.links.clear();
        assert_eq!(
            actions(&links_menu(&bare, task)),
            [MenuAction::AddLink(task)]
        );
    }

    #[test]
    fn a_deleted_list_offers_nothing() {
        let mut f = fixtures::my_day();
        let office = list_named(&f.state, "Office");
        f.state.lists.get_mut(&office).expect("list").deleted = Some(todo_core::model::Tombstone {
            at: f.now.at,
            by: todo_core::Actor::System,
        });
        assert!(list_menu(&f.state, office, Page::Main).is_empty());
        assert!(list_menu(&f.state, office, Page::Confirm).is_empty());
        assert!(room_menu(&f.state, RoomKey::List(office), false, Page::Confirm).is_empty());
    }

    #[test]
    fn the_account_menu_links_the_paired_servers_admin_pages() {
        let menu = account_menu(Some("https://todo.example.com"), false);
        assert_eq!(
            actions(&menu),
            [
                MenuAction::OpenAdmin,
                MenuAction::SyncNow,
                MenuAction::DeviceSettings,
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
        let [
            MenuEntry::Confirm {
                message, buttons, ..
            },
        ] = confirm.as_slice()
        else {
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
            Page::Main,
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
            room_menu(&f.state, RoomKey::List(list), false, Page::Main),
            list_menu(&f.state, list, Page::Main),
            collection_menu(&f.state, col, false, Page::Main),
            account_menu(Some("https://todo.example.com"), false),
            account_menu(Some("https://todo.example.com"), true),
            list_menu(&f.state, list, Page::Confirm),
            room_menu(&f.state, RoomKey::List(list), false, Page::Confirm),
            priority_menu(task, Priority::None),
            task_menu(&f.state, task, f.now.today()),
            move_menu(&f.state, task, ""),
            failures_menu(&f.state, &failed(&f.state, 3)),
            collection_menu(&f.state, col, false, Page::Confirm),
            list_menu(&f.state, list, Page::Move),
            list_menu(&f.state, list, Page::Colour),
            tag_menu(&f.state, task, "x"),
            links_menu(&f.state, task),
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
                MenuAction::PickMerge(id),
                MenuAction::AskLinks(id),
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
    fn merge_picker_rechecks_the_chosen_task_and_defaults_to_cancel() {
        let mut f = fixtures::my_day();
        let source = f.ui.task.expect("selected task");
        let picker = merge_picker(&f.state, source, "");
        assert!(is_wide(&picker));
        let MenuAction::AskMerge(_, target) = items(&picker).next().expect("candidate").action
        else {
            panic!("missing merge destination");
        };
        assert_ne!(source, target);
        let confirm = merge_confirmation(&f.state, source, target);
        assert!(is_confirmation(&confirm));
        assert_eq!(first_highlight(&confirm), Some(1));
        assert_eq!(
            actions(&confirm),
            [MenuAction::Merge(source, target), MenuAction::Cancel]
        );
        let asked = Asked::of(&confirm);
        f.state
            .tasks
            .get_mut(&target)
            .expect("target")
            .title
            .push_str(" later");
        assert!(matches!(
            on_confirm(
                asked.as_ref(),
                &merge_confirmation(&f.state, source, target)
            ),
            Confirmed::AskAgain(_)
        ));
    }

    #[test]
    fn task_menu_toggle_icons_show_current_state_and_priority_keeps_its_flag() {
        let mut f = fixtures::my_day();
        let id = f.ui.task.expect("selected task");
        let today = f.now.today();
        for (on, expected) in [
            (false, [Icon::Sun, Icon::Star, Icon::Flag]),
            (true, [Icon::SunFilled, Icon::StarFilled, Icon::Flag]),
        ] {
            let task = f.state.tasks.get_mut(&id).expect("selected task");
            task.my_day = on.then_some(today);
            task.priority = if on { Priority::High } else { Priority::Normal };
            let menu = task_menu(&f.state, id, today);
            assert_eq!(
                items(&menu)
                    .skip(1)
                    .take(3)
                    .map(|i| i.icon)
                    .collect::<Vec<_>>(),
                expected,
            );
        }
    }

    #[test]
    fn the_list_picker_groups_lists_by_collection_ticks_the_tasks_own_and_filters() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected task");
        let own = f.state.tasks[&task].list;
        let menu = move_menu(&f.state, task, "");
        // The filter line first, then each collection's name above its lists, as in the sidebar.
        assert!(matches!(&menu[0], MenuEntry::Filter { typed: t, .. } if t.is_empty()));
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
        // A list added above the highlighted one (by a sync) leaves the highlight on the same list, and
        // Enter on a list that has gone does nothing.
        let errands_key = format!("move-{}", list_named(&f.state, "Errands"));
        let mut added = f.state.clone();
        let new = ListId::from_uuid(uuid::Uuid::from_u128(77));
        let mut list = added.lists[&list_named(&f.state, "Planning")].clone();
        list.id = new;
        list.name = "Aardvarks".into();
        added.lists.insert(new, list);
        let after = move_menu(&added, task, "");
        assert_eq!(items(&after).count(), live + 1);
        assert_ne!(
            index_of(&after, &errands_key),
            index_of(&menu, &errands_key)
        );
        assert_eq!(
            chosen(&after, Some(&errands_key)),
            Some(MenuAction::MoveTask(task, list_named(&f.state, "Errands")))
        );
        assert_eq!(chosen(&after, Some("move-gone")), None);
        assert_eq!(chosen(&after, None), None);
        let first = key_at(&menu, 0).expect("a key");
        assert_eq!(index_of(&menu, &first), Some(0));
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
        assert!(matches!(&narrowed[0], MenuEntry::Filter { typed: t, .. } if t == "ERRAND"));
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
        // A change refused while the list is open waits for it to open again: the rows, and what each
        // highlighted key does, stay as they were.
        let shown: Vec<CommandId> = failed.iter().map(|f| f.entry.command_id).collect();
        let mut now = failed.clone();
        let mut new = failed[0].clone();
        new.entry.command_id = uuid::Uuid::from_u128(99);
        now.insert(0, new);
        let still = failures_menu(&f.state, &shown_failures(now.clone(), &shown));
        assert_eq!(still, menu);
        let all = failures_menu(&f.state, &now);
        assert_eq!(
            chosen(&all, Some("dismiss-all")),
            Some(MenuAction::DismissAllFailures)
        );
        let second = format!("dismiss-{}", failed[1].entry.command_id);
        assert_eq!(
            chosen(&all, Some(&second)),
            Some(MenuAction::DismissFailure(failed[1].entry.command_id))
        );
        // One dismissed meanwhile is gone from the list.
        let fewer = shown_failures(now[2..].to_vec(), &shown);
        assert_eq!(fewer.len(), 2);
        // One refused change needs no Dismiss all; none, no list.
        let one = failures_menu(&f.state, &failed[..1]);
        assert!(matches!(&one[0], MenuEntry::Heading(h) if h == "1 change couldn't sync"));
        assert_eq!(items(&one).count(), 1);
        assert!(failures_menu(&f.state, &[]).is_empty());
    }
}
