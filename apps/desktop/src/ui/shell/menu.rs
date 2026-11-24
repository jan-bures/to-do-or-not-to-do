//! The popover menus: a list's, a collection's or a task's context menu (at the pointer), and the menus
//! that open from a button (the room header's "more", the account gear, the detail pane's Priority and
//! Tags rows).
//!
//! What a menu offers is data (`model::menus`); this draws it and runs the chosen item. Every menu
//! takes the keyboard while it is open: Up and Down (Left and Right between a confirmation's buttons)
//! walk its items, Enter or Space choose, Escape or Tab close it and give the keyboard back to its
//! button (or the list). It closes whenever the keyboard goes anywhere else (a shortcut, a click into a
//! field, another window). It enters over `OVERLAY_IN` and fades out over `OVERLAY_OUT`
//! (`model::motion::Overlay`); a closing menu is paint-only: it takes no clicks, keys or hovers.

use gpui::{
    Anchor, AnyElement, Context, Div, KeyDownEvent, Pixels, Point, Role, SharedString, Window,
    anchored, deferred, div, prelude::*, px,
};
use todo_client::intent::organize::actionable_task;
use todo_core::ids::{CollectionId, EntityId, ListId, TaskId};

use super::sidebar::InlineTarget;
use super::{Notice, ShellEvent, ShellView};
use crate::model::menus::reminder;
use crate::model::menus::repeat::{self, Draft};
use crate::model::menus::{self, Asked, Confirmed, MenuAction, MenuEntry, MenuItem, Page, TagPage};
use crate::model::motion::Overlay;
use crate::model::{commands, hints::Stop, labels};
use crate::theme::{
    self, Icon, Palette, TextStyled, component, desktop, elevation, radius, space, ty,
};
use crate::ui::kit;

/// How many rows a menu shows before it scrolls (the list picker, a long "couldn't sync" list).
const MENU_ROWS: f32 = 12.;

/// An open (or closing) popover menu.
#[derive(Clone, Debug)]
pub(super) struct Menu {
    pub kind: MenuKind,
    /// A context menu opens at the pointer (or, opened from the keyboard, under its row). A menu with
    /// a button opens from it, drawn inside it, and has none.
    pub at: Option<Point<Pixels>>,
    /// The item the keyboard is on (its key, `menus::key_at`), if it has moved yet. Kept by key, so data
    /// that changes under the menu never moves it to another item; Enter on an item that has gone does
    /// nothing.
    pub highlight: Option<SharedString>,
    pub overlay: Overlay,
    /// Where closing the menu from the keyboard gives the keyboard back: its button.
    pub trigger: Option<Stop>,
    /// What's been typed to narrow the list picker.
    pub filter: String,
    /// The refused changes the "couldn't sync" list shows: those there when it opened (one refused
    /// since shows the next time it opens). Dismiss all takes exactly these.
    pub failures: Vec<todo_client::CommandId>,
    /// A delete confirmation's question as it was drawn when it opened (or was asked again). It stays as
    /// asked while data changes under it, and confirming compares it with what it would say now.
    pub asked: Option<Asked>,
    /// The source the move picker opened on; never infer it again when choosing.
    pub(super) move_from: Option<ListId>,
}

/// What a task's context menu shows: its items, or in their place the priority picker, the list
/// picker or the task's links that one of them opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskPage {
    Main,
    Priority,
    Move,
    PickMerge,
    ConfirmMerge(TaskId),
    Links,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuKind {
    /// The sidebar footer's New button.
    New,
    /// The sidebar footer's account menu: the server, its admin pages, Sync now and Sign out (`confirm` once
    /// asked).
    Account {
        confirm: bool,
    },
    Updates,
    DateFormat,
    /// A list's context menu, or the page one of its items turned it into.
    List {
        id: ListId,
        page: Page,
    },
    /// A built-in smart list's sidebar context menu.
    Smart(crate::model::Smart),
    /// A collection's context menu, or the page one of its items turned it into.
    Collection {
        id: CollectionId,
        page: Page,
    },
    /// The room header's "more" menu, or the page one of its items turned it into.
    Room {
        page: Page,
    },
    /// The tag picker, from the detail pane's Tags row (or T), or a tag's own page in it.
    Tags {
        task: TaskId,
        page: TagPage,
    },
    Date(super::quick_pickers::Calendar),
    QuickPriority,
    Status {
        task: Option<TaskId>,
        at_row: bool,
    },
    /// The detail pane's priority picker.
    Priority {
        task: TaskId,
    },
    Repeat {
        task: TaskId,
        draft: Option<Draft>,
    },
    /// The detail pane's reminder picker: its presets, or (`draft`) its custom page. `seen` is the
    /// reminder the row showed when it opened, which an edit is made on. The presets are resolved
    /// once on opening, so a redraw or keyboard action cannot change their meaning.
    Reminder {
        task: TaskId,
        seen: Option<todo_client::intent::reminder::Seen>,
        draft: Option<reminder::Draft>,
        presets: reminder::Presets,
    },
    /// A task's context menu (right-click its row, or Shift+F10), or the list picker (M).
    Task {
        id: TaskId,
        page: TaskPage,
    },
    /// The list picker from the multi-selection bar.
    BulkMove,
    BulkStatus,
    /// The sidebar footer's "N couldn't sync": the refused changes, to see and dismiss.
    SyncFailures,
}

impl MenuKind {
    /// Which way the menu opens from its anchor: down (shifting in from above) or up.
    fn opens_up(self) -> bool {
        matches!(
            self,
            Self::New
                | Self::Account { .. }
                | Self::Updates
                | Self::DateFormat
                | Self::SyncFailures
                | Self::BulkMove
                | Self::BulkStatus
        )
    }

    /// Whether a button's menu hangs from its left edge rather than its right.
    fn aligns_left(self) -> bool {
        matches!(self, Self::New | Self::SyncFailures)
    }

    /// The same menu, apart from its confirmation step (or the picker a task's menu turned into).
    pub(super) fn same_menu(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::Account { .. } | Self::Updates | Self::DateFormat,
                Self::Account { .. } | Self::Updates | Self::DateFormat,
            )
            | (Self::Room { .. }, Self::Room { .. }) => true,
            (Self::Date(a), Self::Date(b)) => {
                a.task == b.task && (a.task.is_none() || a.starts == b.starts)
            }
            (Self::List { id: a, .. }, Self::List { id: b, .. }) => a == b,
            (Self::Collection { id: a, .. }, Self::Collection { id: b, .. }) => a == b,
            (Self::Task { id: a, .. }, Self::Task { id: b, .. }) => a == b,
            (Self::Tags { task: a, .. }, Self::Tags { task: b, .. })
            | (Self::Repeat { task: a, .. }, Self::Repeat { task: b, .. })
            | (Self::Reminder { task: a, .. }, Self::Reminder { task: b, .. }) => a == b,
            _ => self == other,
        }
    }

    /// The task a context menu or picker is about.
    pub(super) fn task(self) -> Option<TaskId> {
        match self {
            Self::Task { id, .. } => Some(id),
            Self::Status { task, .. } => task,
            _ => None,
        }
    }

    /// Whether what's typed goes to the menu's filter line: the list picker and the tag picker.
    fn filters(self) -> bool {
        matches!(
            self,
            Self::Task {
                page: TaskPage::Move | TaskPage::PickMerge,
                ..
            } | Self::BulkMove
                | Self::Tags {
                    page: TagPage::Pick,
                    ..
                }
        )
    }

    /// The same menu on another page (a confirmation, a picker, the colour swatches), where it has one.
    fn settings(self) -> bool {
        matches!(
            self,
            Self::Room {
                page: Page::Settings
            } | Self::List {
                page: Page::Settings,
                ..
            }
        )
    }

    fn on_page(self, page: Page) -> Self {
        match self {
            Self::List { id, .. } => Self::List { id, page },
            Self::Collection { id, .. } => Self::Collection { id, page },
            Self::Room { .. } => Self::Room { page },
            other => other,
        }
    }
}

impl ShellView {
    /// What the menu offers now: a delete confirmation as it was asked (`menus::as_asked`).
    pub(super) fn menu_entries(&self, kind: MenuKind) -> Vec<MenuEntry> {
        let entries = self.live_menu_entries(kind);
        match self.menu.as_ref().filter(|m| m.kind == kind) {
            Some(Menu {
                asked: Some(asked), ..
            }) if !entries.is_empty() => menus::as_asked(entries, asked),
            _ => entries,
        }
    }

    /// What the menu would offer from the data now.
    fn live_menu_entries(&self, kind: MenuKind) -> Vec<MenuEntry> {
        let state = self.store.view();
        match kind {
            MenuKind::New => menus::new_menu(),
            MenuKind::Account { confirm } => {
                let mut entries = menus::account_menu(self.store.server().as_deref(), confirm);
                if !confirm {
                    let before_updates = entries.iter().position(|entry| matches!(entry, MenuEntry::Item(i) if i.action == MenuAction::OpenUpdates)).unwrap_or(entries.len());
                    entries.insert(
                        before_updates,
                        menus::date_format_entry(self.store.now().dates()),
                    );
                }
                entries
            }
            MenuKind::DateFormat => menus::date_format_menu(self.store.now().dates()),
            MenuKind::Updates => Self::update_entries(&self.update_snapshot()),
            MenuKind::List { id, page } => menus::list_menu(&state, id, page),
            MenuKind::Smart(smart) => menus::smart_context_menu(smart),
            MenuKind::Collection { id, page } => menus::collection_menu(
                &state,
                id,
                self.ui.collapsed_collections.contains(&id),
                page,
            ),
            MenuKind::Room { page } => self.room_entries(&state, page),
            MenuKind::Tags {
                task,
                page: TagPage::Pick,
            } => {
                let filter = self.menu.as_ref().map_or("", |m| m.filter.as_str());
                menus::tag_menu(&state, task, filter)
            }
            MenuKind::Tags {
                task,
                page: TagPage::Edit(tag),
            } => menus::tag_edit_menu(&state, task, tag),
            MenuKind::Tags {
                task,
                page: TagPage::Confirm(tag),
            } => menus::tag_confirm(&state, task, tag),
            MenuKind::Repeat { task, draft } => state
                .tasks
                .get(&task)
                .filter(|_| state.is_live(task.into()))
                .map(|t| repeat::entries(t, self.store.now().dates(), draft))
                .unwrap_or_default(),
            MenuKind::Reminder {
                task,
                seen,
                draft,
                ref presets,
            } if state.is_live(task.into()) => {
                reminder::entries(task, seen.map(|s| s.at), draft, presets, &self.store.now())
            }
            MenuKind::Reminder { .. } => Vec::new(),
            MenuKind::Tags {
                page: TagPage::Colour(tag),
                ..
            } => menus::colour_menu(&state, tag.into()),
            MenuKind::Task {
                id,
                page: TaskPage::Links,
            } => menus::links_menu(&state, id),
            MenuKind::Date(_) => Vec::new(),
            MenuKind::Status { task, .. } => match task {
                Some(id) => self
                    .task_stage(id)
                    .map_or_else(Vec::new, |stage| menus::status_entries(task, stage)),
                None if self.quick_features().workflow => {
                    menus::status_entries(None, self.quick_stage())
                }
                None => Vec::new(),
            },
            MenuKind::QuickPriority => menus::priority_entries(
                self.quick_preview().priority.unwrap_or_default(),
                MenuAction::QuickPriority,
            ),
            MenuKind::Priority { task }
            | MenuKind::Task {
                id: task,
                page: TaskPage::Priority,
            } => state
                .tasks
                .get(&task)
                .filter(|_| state.is_live(task.into()))
                .map(|t| menus::priority_menu(task, t.priority))
                .unwrap_or_default(),
            MenuKind::Task {
                id,
                page: TaskPage::Main,
            } if self.ui.compact => menus::popout_task_menu(&state, id, self.store.now().today()),
            MenuKind::Task {
                id,
                page: TaskPage::Main,
            } => menus::task_menu(&state, id, self.store.now().today()),
            MenuKind::Task {
                id,
                page: TaskPage::Move,
            } => {
                let menu = self.menu.as_ref().filter(|m| m.kind == kind);
                if menu.is_some_and(|m| actionable_task(&state, id).map(|t| t.list) != m.move_from)
                {
                    return Vec::new();
                }
                menus::move_menu(&state, id, menu.map_or("", |m| m.filter.as_str()))
            }
            MenuKind::Task {
                id,
                page: TaskPage::PickMerge,
            } => {
                let filter = self
                    .menu
                    .as_ref()
                    .filter(|m| m.kind == kind)
                    .map_or("", |m| m.filter.as_str());
                menus::merge_picker(&state, id, filter)
            }
            MenuKind::Task {
                id,
                page: TaskPage::ConfirmMerge(target),
            } => menus::merge_confirmation(&state, id, target),
            MenuKind::BulkStatus if self.on_board() && self.bulk_ids().len() >= 2 => {
                menus::bulk_status_entries()
            }
            MenuKind::BulkStatus => Vec::new(),
            MenuKind::BulkMove => {
                let filter = self
                    .menu
                    .as_ref()
                    .filter(|m| m.kind == kind)
                    .map_or("", |m| m.filter.as_str());
                menus::bulk_move_menu(&state, &self.bulk_ids(), filter)
            }
            MenuKind::SyncFailures => {
                let shown = self
                    .menu
                    .as_ref()
                    .map_or(&[][..], |m| m.failures.as_slice());
                let failed = menus::shown_failures(self.store.failed(), shown);
                menus::failures_menu(&state, &failed)
            }
        }
    }

    /// Whether the room header shows its "more" button: only when its menu offers something.
    pub(super) fn has_room_menu(&self) -> bool {
        !self.room_entries(&self.store.view(), Page::Main).is_empty()
    }

    /// The room's menu: Activity's (refresh, and whose changes it shows), or a task room's.
    fn room_entries(&self, state: &todo_core::State, page: Page) -> Vec<MenuEntry> {
        match self.shell.room.key {
            crate::model::RoomKey::Activity(agent) if page == Page::Main => {
                menus::activity_menu(&self.shell.sidebar.agents, agent)
            }
            room => {
                let mut entries = menus::room_menu(state, room, self.ui.show_completed, page);
                if self.on_board() {
                    entries.retain(|e| !matches!(e, MenuEntry::Item(i) if i.action == MenuAction::ToggleCompleted));
                }
                entries
            }
        }
    }

    /// Opens a menu, or turns the open one into `kind` (its confirmation step) in place. The keyboard
    /// goes to the menu; `trigger` is where it goes back.
    pub(super) fn open_menu(
        &mut self,
        kind: MenuKind,
        at: Option<Point<Pixels>>,
        trigger: Option<Stop>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.now();
        let open = self
            .menu
            .as_ref()
            .filter(|m| m.overlay.look(now).open && m.kind.same_menu(kind));
        let animate = self.animate;
        let pace = self.pace();
        let menu = match open {
            // The same menu on its next page: the page fades in where the menu stands.
            Some(m) => {
                let mut overlay = m.overlay;
                if animate && m.kind != kind {
                    overlay.swap_content(now, pace);
                }
                Menu {
                    kind,
                    overlay,
                    filter: String::new(),
                    ..m.clone()
                }
            }
            None => Menu {
                kind,
                at,
                highlight: None,
                overlay: if self.animate {
                    Overlay::open(now, self.pace())
                } else {
                    Overlay::rest()
                },
                trigger,
                filter: String::new(),
                asked: None,
                move_from: None,
                failures: if kind == MenuKind::SyncFailures {
                    (self.store.failed().iter())
                        .map(|f| f.entry.command_id)
                        .collect()
                } else {
                    Vec::new()
                },
            },
        };
        // A keyboard user stays on the keyboard: a confirmation starts on Cancel.
        let keyboard = menu.highlight.is_some();
        // A confirmation keeps its question as asked now.
        let asked = Asked::of(&self.live_menu_entries(kind));
        let move_from = match kind {
            MenuKind::Task {
                page: TaskPage::Move,
                ..
            } if open.is_some_and(|m| m.kind == kind) => menu.move_from,
            MenuKind::Task {
                id,
                page: TaskPage::Move,
            } => actionable_task(&self.store.view(), id).map(|t| t.list),
            _ => None,
        };
        self.menu = Some(Menu {
            asked,
            move_from,
            ..menu
        });
        // A new menu, or a new page of it, starts scrolled to its top.
        self.menu_scroll = kit::ScrollPane::default();
        if keyboard {
            self.highlight_first();
        }
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    /// Opens the menu from its button, or leaves it closed when the press of this very click closed it
    /// (pressing the button of an open menu lands outside the menu first, which closes it). Only that
    /// press counts (`PressGuard`), never the menu's fade: a later click on the button, after a
    /// dismissal anywhere (even on an occluding pane) or while the menu still fades, opens it again.
    pub(super) fn toggle_menu(
        &mut self,
        kind: MenuKind,
        trigger: Stop,
        click: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keyboard = click.mouse_position().is_none();
        if !keyboard && self.menu_press.swallows(kind) {
            cx.notify();
            return;
        }
        self.open_menu(kind, None, Some(trigger), window, cx);
        // Opened from the keyboard (Enter or Space on the button): the keyboard starts on an item.
        if keyboard {
            self.highlight_first();
        }
    }

    /// Puts the keyboard on the open menu's first choice (`menus::first_highlight`).
    pub(super) fn highlight_first(&mut self) {
        let Some(kind) = self.menu.as_ref().map(|m| m.kind) else {
            return;
        };
        let at = menus::first_highlight(&self.menu_entries(kind));
        self.highlight(at);
    }

    /// Puts the keyboard on item `at` of the open menu, scrolling it into view in a long menu.
    pub(super) fn highlight(&mut self, at: Option<usize>) {
        let Some(kind) = self.menu.as_ref().map(|m| m.kind) else {
            return;
        };
        let entries = self.menu_entries(kind);
        let child = at.and_then(|i| menus::child_of_item(&entries, i));
        if let Some(menu) = self.menu.as_mut() {
            menu.highlight = at.and_then(|i| menus::key_at(&entries, i));
        }
        if let Some(child) = child {
            self.menu_scroll.handle.scroll_to_item(child);
        }
    }

    /// The keyboard left the open menu: a shortcut (Ctrl+K, Ctrl+N), a click into a field, or another
    /// window. The menu closes rather than stay open without the keys that close it. When the window
    /// itself went to the background, the keyboard comes back to the menu's button.
    pub(super) fn menu_lost_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = (self.menu.as_ref()).is_some_and(|m| m.overlay.look(self.now()).open);
        // Focus went back to the menu itself (a stale event), and the window is still active: stay.
        if !open || (self.menu_focus.is_focused(window) && window.is_window_active()) {
            return;
        }
        self.close_menu(true, window, cx);
    }

    /// Closes the menu: it fades out (paint-only), or goes at once when nothing animates. With
    /// `refocus`, the keyboard goes back to its button, else to the list.
    pub(super) fn close_menu(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (now, pace) = (self.now(), self.pace());
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let trigger = menu.trigger;
        if self.animate {
            menu.overlay.close(now, pace);
        } else {
            self.menu = None;
        }
        if self.menu_focus.is_focused(window) {
            match trigger.filter(|_| refocus) {
                Some(stop) => self.focus_stop(stop, window, cx),
                None => window.focus(&self.list_focus, cx),
            }
        }
        cx.notify();
    }

    /// Drops a menu that has faded out; true while it still needs frames.
    pub(super) fn settle_menu(&mut self) -> bool {
        let now = self.now();
        if self.menu.as_ref().is_some_and(|m| m.overlay.gone(now)) {
            self.menu = None;
        }
        self.menu
            .as_ref()
            .is_some_and(|m| m.overlay.wake(now) != crate::model::motion::Wake::Idle)
    }

    /// The keys of a menu button (the room's "more", the account gear): Escape goes back to the list.
    /// Enter and Space click it (GPUI's own keyboard click); Tab is the shell's (`ShellView::tab_key`).
    pub(super) fn control_key(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
            cx.stop_propagation();
            window.focus(&self.list_focus, cx);
            cx.notify();
        }
    }

    fn menu_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self
            .menu
            .as_ref()
            .filter(|m| m.overlay.look(self.frame).open)
            .cloned()
        else {
            return;
        };
        let key = e.keystroke.key.as_str();
        if e.keystroke.modifiers.control || e.keystroke.modifiers.alt {
            return;
        }
        cx.stop_propagation();
        if let MenuKind::Date(calendar) = menu.kind {
            self.calendar_key(calendar, e, window, cx);
            return;
        }
        let entries = self.menu_entries(menu.kind);
        let count = menus::items(&entries).count();
        if let Some(Menu {
            kind: MenuKind::Repeat { draft: Some(d), .. },
            ..
        }) = self.menu.as_mut()
            && d.type_key(key)
        {
            cx.notify();
            return;
        }
        // The custom reminder page: digits type the time.
        if let Some(Menu {
            kind: MenuKind::Reminder { draft: Some(d), .. },
            ..
        }) = self.menu.as_mut()
            && !e.is_held
            && d.type_key(key)
        {
            cx.notify();
            return;
        }
        if let Some(filter) = picker_edit(&menu, e) {
            if let Some(m) = self.menu.as_mut() {
                m.filter = filter;
            }
            // The keyboard goes to the first list that matches, at the top.
            let first = menus::items(&self.menu_entries(menu.kind))
                .next()
                .map(|_| 0);
            self.menu_scroll = kit::ScrollPane::default();
            self.highlight(first);
            cx.notify();
            return;
        }
        let at = (menu.highlight.as_deref()).and_then(|k| menus::index_of(&entries, k));
        // Right opens a tag's own page (its chevron); Left on that page goes back to every tag.
        let more = at
            .and_then(|i| menus::items(&entries).nth(i))
            .and_then(|i| i.more);
        let back = match menu.kind {
            MenuKind::Tags {
                task,
                page: TagPage::Edit(_),
            } => Some(MenuAction::AllTags(task)),
            _ => None,
        };
        match key {
            "right" if more.is_some() => {
                if let Some(action) = more {
                    self.run_menu_action(action, true, window, cx);
                }
            }
            "left" if back.is_some() => {
                if let Some(action) = back {
                    self.run_menu_action(action, true, window, cx);
                }
            }
            // The colour swatches are a grid: Up and Down move a row.
            "down" | "up" | "right" | "left" if menus::is_grid(&entries) => {
                self.highlight(menus::grid_step(at, key, count));
                cx.notify();
            }
            "down" | "up" | "right" | "left" => {
                let by = if matches!(key, "down" | "right") {
                    1
                } else {
                    -1
                };
                let next = menus::step_highlight(at, by, count);
                self.highlight(next);
                cx.notify();
            }
            "home" | "end" if count > 0 => {
                let at = if key == "home" { 0 } else { count - 1 };
                self.highlight(Some(at));
                cx.notify();
            }
            "enter" | "space" if !e.is_held => {
                // The highlighted item as the menu shows it now; nothing if it has gone meanwhile.
                if let Some(action) = menus::chosen(&entries, menu.highlight.as_deref()) {
                    self.run_menu_action(action, true, window, cx);
                }
            }
            "escape" if menu.kind.settings() => {
                self.run_menu_action(MenuAction::CloseListSettings, true, window, cx)
            }
            "escape" if menu.kind == MenuKind::DateFormat => {
                self.run_menu_action(MenuAction::CloseDateFormat, true, window, cx)
            }
            "escape" if menu.kind == MenuKind::Updates && menu.trigger == Some(Stop::Account) => {
                self.run_menu_action(MenuAction::CloseUpdates, true, window, cx)
            }
            "escape" | "tab" => self.close_menu(true, window, cx),
            _ => {}
        }
    }

    /// Does what a menu item says. `keyboard`: it was chosen with the keyboard, so a confirmation that
    /// follows starts highlighted, and an item that opens nothing of its own gives the keyboard back to
    /// the menu's button (`MenuAction::moves_focus`).
    fn run_menu_action(
        &mut self,
        action: MenuAction,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menu.clone() else {
            return;
        };
        let entries = self.menu_entries(menu.kind);
        if keyboard
            && menu.highlight.is_none()
            && let Some(m) = self.menu.as_mut()
        {
            m.highlight = menus::items(&entries)
                .find(|i| i.action == action)
                .map(|i| i.key.clone());
        }
        match action {
            MenuAction::QuickPriority(level) => {
                return self.quick_pick(super::quick_pickers::Pick::Priority(level), window, cx);
            }
            MenuAction::Status(task, stage) => {
                if let Some(task) = task {
                    self.close_menu(keyboard, window, cx);
                    self.choose_status(task, stage, cx);
                } else {
                    self.quick_pick(
                        super::quick_pickers::Pick::Status(super::workflow::open_stage(stage)),
                        window,
                        cx,
                    );
                }
                return;
            }
            MenuAction::OpenDateFormat
            | MenuAction::CloseDateFormat
            | MenuAction::SetDateFormat(_) => {
                let opening = action == MenuAction::OpenDateFormat;
                if let MenuAction::SetDateFormat(format) = action {
                    match self.store.set_date_format(format) {
                        Ok(()) => self.refresh(cx),
                        Err(e) => {
                            tracing::warn!(%e, "couldn't save date format");
                            self.notice = Some(Notice::error("Couldn't save date format."));
                        }
                    }
                }
                let kind = if opening {
                    MenuKind::DateFormat
                } else {
                    MenuKind::Account { confirm: false }
                };
                self.open_menu(kind, menu.at, menu.trigger, window, cx);
                if keyboard {
                    let key = if opening {
                        self.store.now().date_format.key()
                    } else {
                        "date-format"
                    };
                    self.highlight(menus::index_of(&self.menu_entries(kind), key));
                }
                return;
            }
            MenuAction::OpenUpdates | MenuAction::CloseUpdates => {
                let opening = action == MenuAction::OpenUpdates;
                let kind = if opening {
                    MenuKind::Updates
                } else {
                    MenuKind::Account { confirm: false }
                };
                self.open_menu(kind, menu.at, menu.trigger, window, cx);
                if !opening && keyboard {
                    self.highlight(menus::index_of(&self.menu_entries(kind), "updates"));
                }
                return;
            }
            MenuAction::CheckUpdates => {
                if let Some(updates) = &self.updates {
                    updates.check_now();
                    cx.notify();
                }
                return;
            }
            MenuAction::RestartToUpdate => {
                cx.emit(ShellEvent::RestartToUpdate);
                return;
            }
            MenuAction::OpenListSettings | MenuAction::CloseListSettings => {
                let page = if action == MenuAction::OpenListSettings {
                    Page::Settings
                } else {
                    Page::Main
                };
                self.open_menu(menu.kind.on_page(page), menu.at, menu.trigger, window, cx);
                if page == Page::Main {
                    let entries = self.menu_entries(menu.kind.on_page(Page::Main));
                    self.highlight(menus::index_of(&entries, "list-settings"));
                } else if keyboard {
                    self.highlight_first();
                }
                return;
            }
            MenuAction::ListSetting(id, setting) => {
                self.apply_list_setting(id, setting, cx);
                return;
            }
            // The menu turns into the confirmation, the collection picker or the swatches, in place.
            MenuAction::Repeat(task, a) => {
                return self.repeat_action(task, a, keyboard, window, cx);
            }
            MenuAction::Reminder(task, a) => {
                return self.reminder_action(task, a, keyboard, window, cx);
            }
            MenuAction::AskDeleteList(_)
            | MenuAction::AskDeleteCollection(_)
            | MenuAction::AskDeleteTag(_)
            | MenuAction::AskMoveList(_)
            | MenuAction::AskColour(_) => {
                let kind = match (menu.kind, action) {
                    (MenuKind::Tags { task, .. }, MenuAction::AskColour(EntityId::Tag(tag))) => {
                        MenuKind::Tags {
                            task,
                            page: TagPage::Colour(tag),
                        }
                    }
                    (MenuKind::Tags { task, .. }, MenuAction::AskDeleteTag(tag)) => {
                        MenuKind::Tags {
                            task,
                            page: TagPage::Confirm(tag),
                        }
                    }
                    (kind, MenuAction::AskMoveList(_)) => kind.on_page(Page::Move),
                    (kind, MenuAction::AskColour(_)) => kind.on_page(Page::Colour),
                    (kind, _) => kind.on_page(Page::Confirm),
                };
                self.open_menu(kind, menu.at, menu.trigger, window, cx);
                // A picker starts on the current choice, from the keyboard or not.
                if !matches!(
                    action,
                    MenuAction::AskDeleteList(_)
                        | MenuAction::AskDeleteCollection(_)
                        | MenuAction::AskDeleteTag(_)
                ) {
                    self.highlight_first();
                }
                return;
            }
            MenuAction::AskSignOut => {
                return self.open_menu(
                    MenuKind::Account { confirm: true },
                    menu.at,
                    menu.trigger,
                    window,
                    cx,
                );
            }
            // A task's menu turns into the picker in place; the keyboard starts on the current choice.
            MenuAction::AskPriority(id)
            | MenuAction::AskMove(id)
            | MenuAction::AskLinks(id)
            | MenuAction::PickMerge(id)
            | MenuAction::AskMerge(id, _) => {
                let page = match action {
                    MenuAction::AskMove(_) => TaskPage::Move,
                    MenuAction::PickMerge(_) => TaskPage::PickMerge,
                    MenuAction::AskMerge(_, target) => TaskPage::ConfirmMerge(target),
                    MenuAction::AskLinks(_) => TaskPage::Links,
                    _ => TaskPage::Priority,
                };
                let kind = MenuKind::Task { id, page };
                self.open_menu(kind, menu.at, menu.trigger, window, cx);
                self.highlight_first();
                return;
            }
            // The tag picker and a tag's own page turn into each other in place.
            MenuAction::EditTag(task, _) | MenuAction::AllTags(task) => {
                let page = match action {
                    MenuAction::EditTag(_, tag) => TagPage::Edit(tag),
                    _ => TagPage::Pick,
                };
                let back_to = match (menu.kind, action) {
                    (
                        MenuKind::Tags {
                            page: TagPage::Edit(tag) | TagPage::Colour(tag),
                            ..
                        },
                        MenuAction::AllTags(_),
                    ) => Some(format!("tag-{tag}")),
                    _ => None,
                };
                self.open_menu(
                    MenuKind::Tags { task, page },
                    menu.at,
                    menu.trigger,
                    window,
                    cx,
                );
                // Back on every tag, the keyboard is on the tag it came from.
                match back_to {
                    Some(key) => {
                        let entries = self.menu_entries(MenuKind::Tags { task, page });
                        self.highlight(menus::index_of(&entries, &key));
                    }
                    None if keyboard || menu.highlight.is_some() => self.highlight_first(),
                    None => {}
                }
                cx.notify();
                return;
            }
            // Tags go on and off with the picker open, the keyboard staying on the same tag.
            MenuAction::ToggleTag(task, tag) => {
                if let Some(cmd) = commands::toggle_tag(&self.store.view(), task, tag) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
                return;
            }
            MenuAction::CreateTag(task) => {
                self.create_tag(task, &menu, cx);
                return;
            }
            // The list stays open while more are left, the keyboard on the item that took its place.
            MenuAction::DismissFailure(id) => {
                let at = (menu.highlight.as_deref()).and_then(|k| menus::index_of(&entries, k));
                self.dismiss_failure(Some(id), &menu);
                let left = menus::items(&self.menu_entries(menu.kind)).count();
                if left > 0 {
                    self.highlight(at.map(|i| i.min(left - 1)));
                    self.refresh(cx);
                    return;
                }
            }
            _ => {}
        }
        // A delete is confirmed only for what the question said: if the names or counts changed while
        // it was open, nothing is deleted and the new question is asked, the keyboard on Cancel.
        if matches!(
            action,
            MenuAction::DeleteList(_)
                | MenuAction::DeleteCollection(_)
                | MenuAction::DeleteTag(_)
                | MenuAction::Merge(..)
        ) {
            match menus::on_confirm(menu.asked.as_ref(), &self.live_menu_entries(menu.kind)) {
                Confirmed::Gone => return self.close_menu(false, window, cx),
                Confirmed::Proceed => {}
                Confirmed::AskAgain(question) => {
                    let (now, pace) = (self.now(), self.pace());
                    if let Some(m) = self.menu.as_mut() {
                        m.asked = Some(Asked {
                            question,
                            changed: true,
                        });
                        if self.animate {
                            m.overlay.swap_content(now, pace);
                        }
                    }
                    self.highlight_first();
                    cx.notify();
                    return;
                }
            }
        }
        self.close_menu(keyboard && !action.moves_focus(), window, cx);
        match action {
            MenuAction::RenameList(id) => self.rename_list(id, window, cx),
            MenuAction::RenameCollection(id) => {
                let name = self
                    .store
                    .view()
                    .collections
                    .get(&id)
                    .map(|c| SharedString::from(labels::one_line(&c.name)));
                if let Some(name) = name {
                    self.start_inline(InlineTarget::Collection(id), name, window, cx);
                }
            }
            MenuAction::ToggleCompleted => {
                self.ui.show_completed = !self.ui.show_completed;
                self.refresh(cx);
            }
            MenuAction::DeleteTag(id) => {
                self.submit(vec![commands::delete(id)]);
                self.refresh(cx);
            }
            MenuAction::DeleteList(id) => {
                self.submit(vec![commands::delete(id)]);
                self.refresh(cx);
            }
            MenuAction::DeleteCollection(id) => {
                if let Some(cmd) = commands::delete_collection(&self.store.view(), id) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::MoveList(id, to) => self.move_list(id, to, cx),
            MenuAction::SetColour(id, key) => {
                if let Some(cmd) = commands::set_color(&self.store.view(), id, key) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::RenameTag(tag) => {
                let name = (self.store.view().tags.get(&tag))
                    .map(|t| SharedString::from(labels::one_line(&t.name)));
                if let Some(name) = name {
                    self.start_inline(InlineTarget::Tag(tag), name, window, cx);
                }
            }
            MenuAction::AddLink(task) => self.start_link(task, window, cx),
            MenuAction::RemoveLink(link) => {
                if let Some(cmd) = commands::delete_link(&self.store.view(), link) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::Nudge(id, by) => self.nudge(id, by, cx),
            MenuAction::ToggleCollection(id) => self.toggle_collection(id, cx),
            MenuAction::NewList => self.new_list(None, window, cx),
            MenuAction::NewCollection => self.new_collection(window, cx),
            MenuAction::NewListIn(id) => self.new_list(Some(id), window, cx),
            MenuAction::OpenAdmin => self.open_admin(cx),
            MenuAction::AssistantSettings => {
                window.focus(&self.account_focus, cx);
                self.open_ai_settings(window, cx);
            }
            MenuAction::DeviceSettings => {
                self.open_pairing_panel(super::device_pairing::Mode::Settings, window, cx);
            }
            MenuAction::SyncNow => self.sync_now(cx),
            MenuAction::SignOut => cx.emit(ShellEvent::SignOut),
            MenuAction::PopOut(room) => cx.emit(ShellEvent::PopOut(room)),
            MenuAction::ShowTaskInMain(task) => self.show_task_in_main(task, cx),
            MenuAction::SetPriority(task, level) => {
                if let Some(cmd) = commands::pick_priority(&self.store.view(), task, level) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::ToggleDone(task) => {
                if self.task_is_visible(task) {
                    self.toggle_complete(task, cx);
                }
            }
            MenuAction::ToggleMyDay(task) => {
                let today = self.store.now().today();
                if let Some(cmd) = commands::toggle_my_day(&self.store.view(), task, today) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::ToggleImportant(task) => {
                if let Some(cmd) = commands::toggle_important(&self.store.view(), task) {
                    self.submit(vec![cmd]);
                }
                self.refresh(cx);
            }
            MenuAction::BulkStatus(stage) => {
                let action = super::workflow::open_stage(stage).map_or(
                    todo_core::command::BulkTaskAction::Completion { done: true },
                    |status| todo_core::command::BulkTaskAction::Status { status },
                );
                self.apply_bulk(action, window, cx);
            }
            MenuAction::BulkMove(list) => {
                self.apply_bulk(
                    todo_client::selection::BulkTaskAction::Move { list },
                    window,
                    cx,
                );
            }
            MenuAction::MoveTask(task, list) => {
                if let Some(from) = menu.move_from {
                    self.move_task_to(task, from, list, cx);
                }
            }
            MenuAction::DeleteTask(task) => self.delete_task(task, cx),
            MenuAction::Merge(source, target) => {
                if let Some(asked) = menu.asked {
                    self.merge_task(source, target, &asked.question, cx);
                }
            }
            MenuAction::DismissAllFailures => {
                self.dismiss_failure(None, &menu);
                self.refresh(cx);
            }
            MenuAction::DismissFailure(_) => self.refresh(cx),
            MenuAction::RefreshActivity => self.refresh_activity(cx),
            MenuAction::FilterActivity(agent) => {
                self.select_room(crate::model::RoomKey::Activity(agent), cx);
            }
            MenuAction::AskDeleteList(_)
            | MenuAction::AskDeleteCollection(_)
            | MenuAction::AskMoveList(_)
            | MenuAction::AskColour(_)
            | MenuAction::AskSignOut
            | MenuAction::AskDeleteTag(_)
            | MenuAction::Repeat(..)
            | MenuAction::Reminder(..)
            | MenuAction::AskPriority(_)
            | MenuAction::AskMove(_)
            | MenuAction::PickMerge(_)
            | MenuAction::AskMerge(..)
            | MenuAction::AskLinks(_)
            | MenuAction::EditTag(..)
            | MenuAction::AllTags(_)
            | MenuAction::ToggleTag(..)
            | MenuAction::CreateTag(_)
            | MenuAction::QuickPriority(_)
            | MenuAction::Status(..)
            | MenuAction::OpenUpdates
            | MenuAction::CloseUpdates
            | MenuAction::CheckUpdates
            | MenuAction::RestartToUpdate
            | MenuAction::OpenListSettings
            | MenuAction::CloseListSettings
            | MenuAction::OpenDateFormat
            | MenuAction::CloseDateFormat
            | MenuAction::SetDateFormat(_)
            | MenuAction::ListSetting(..)
            | MenuAction::Cancel => {}
        }
    }

    fn repeat_action(
        &mut self,
        task: TaskId,
        a: repeat::Action,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Menu {
            kind: MenuKind::Repeat { task: id, draft },
            trigger,
            ..
        }) = self.menu.as_ref()
        else {
            return;
        };
        if *id != task {
            return;
        }
        let today = self.store.now().today();
        let state = self.store.view();
        let Some(t) = state
            .tasks
            .get(&task)
            .filter(|_| state.is_live(task.into()))
        else {
            return self.close_menu(false, window, cx);
        };
        let rule = match a {
            repeat::Action::Custom => {
                self.open_menu(
                    MenuKind::Repeat {
                        task,
                        draft: Some(Draft::new(t, today)),
                    },
                    None,
                    *trigger,
                    window,
                    cx,
                );
                return;
            }
            repeat::Action::Preset(p) => {
                p.map(|p| p.rule_for(t.due.unwrap_or(today), t.recurrence.as_ref()))
            }
            repeat::Action::Apply => {
                let Some(d) = draft else { return };
                Some(d.rule())
            }
            edit => {
                if let Some(Menu {
                    kind: MenuKind::Repeat { draft: Some(d), .. },
                    ..
                }) = self.menu.as_mut()
                {
                    d.edit(edit);
                    cx.notify();
                }
                return;
            }
        };
        match commands::repeat::set_repeat(&state, task, rule, today) {
            Ok(cmd) => {
                if let Some(cmd) = cmd
                    && !self.submit(vec![cmd])
                {
                    return self.refresh(cx);
                }
                self.close_menu(keyboard, window, cx);
            }
            Err(error) => self.notice = Some(Notice::error(commands::repeat::error_text(error))),
        }
        self.refresh(cx);
    }

    /// The reminder picker's items: a preset or Set reminder sets the reminder the row showed (or adds
    /// one), Remove takes it away, Custom turns the picker into its custom page, and the steps change
    /// that page's draft. A reminder that changed while the picker was open is never edited: the status
    /// bar says so and the picker shows it as it is now.
    fn reminder_action(
        &mut self,
        task: TaskId,
        a: reminder::Action,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Menu {
            kind:
                MenuKind::Reminder {
                    task: id,
                    seen,
                    draft,
                    presets,
                },
            trigger,
            ..
        }) = self.menu.clone()
        else {
            return;
        };
        if id != task {
            return;
        }
        let state = self.store.view();
        let now = self.store.now();
        let Some(t) = state
            .tasks
            .get(&task)
            .filter(|_| state.is_live(task.into()))
        else {
            return self.close_menu(false, window, cx);
        };
        let at = match a {
            reminder::Action::Custom => {
                let draft = reminder::Draft::new(seen.map(|s| s.at), t.due, &now);
                let kind = MenuKind::Reminder {
                    task,
                    seen,
                    draft: Some(draft),
                    presets,
                };
                self.open_menu(kind, None, trigger, window, cx);
                return;
            }
            reminder::Action::Day(_) | reminder::Action::Minutes(_) => {
                if let Some(Menu {
                    kind: MenuKind::Reminder { draft: Some(d), .. },
                    ..
                }) = self.menu.as_mut()
                {
                    d.edit(a, now.today());
                    cx.notify();
                }
                return;
            }
            // The instant the row showed, never one worked out again now.
            reminder::Action::Preset(_, at) => {
                todo_core::time::Timestamp::from_millisecond(at).ok()
            }
            reminder::Action::Apply => {
                draft.and_then(|d| reminder::preview(d, &now).ok().map(|p| p.0))
            }
            reminder::Action::Remove => None,
        };
        // The stale check (`Seen`) compares with this computer's replica: it covers changes already
        // synced here. One made elsewhere that hasn't arrived yet (this computer offline) isn't seen,
        // and the server applies the later write, as for every other field edit.
        let cmd = match (a, at) {
            (reminder::Action::Remove, _) => match seen {
                Some(seen) => commands::reminder::remove_reminder(&state, task, seen).map(Some),
                None => Ok(None),
            },
            (_, Some(at)) if at > now.at => {
                let store = self.store.clone();
                commands::reminder::set_reminder(&state, task, seen, at, || store.new_uuid())
            }
            _ => {
                self.notice = Some(Notice::error(reminder::PAST));
                return self.refresh(cx);
            }
        };
        match cmd {
            Ok(Some(cmd)) => match self.submit_if(cmd, |state| {
                commands::reminder::check_seen(state, task, seen).is_ok()
            }) {
                Ok(true) => self.close_menu(keyboard, window, cx),
                Ok(false) => self.refresh_stale_reminder(task, window, cx),
                Err(()) => {} // Keep the draft and the storage/command error.
            },
            Ok(None) => self.close_menu(keyboard, window, cx),
            Err(commands::reminder::Stale::Task) => return self.close_menu(false, window, cx),
            Err(commands::reminder::Stale::Reminder) => {
                self.refresh_stale_reminder(task, window, cx);
            }
        }
        self.refresh(cx);
    }

    /// Re-read after a stale snapshot or a refused queue guard, not from the earlier edit snapshot.
    fn refresh_stale_reminder(
        &mut self,
        task: TaskId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        if !state.is_live(task.into()) {
            return self.close_menu(false, window, cx);
        }
        let now = self.store.now();
        self.notice = Some(Notice::error(reminder::CHANGED));
        if let Some(menu) = self.menu.as_mut() {
            menu.kind = MenuKind::Reminder {
                task,
                seen: commands::reminder::Seen::of(&state, task, &now.eval()),
                draft: None,
                presets: reminder::Presets::new(&now),
            };
        }
        self.highlight_first();
    }

    /// Takes one refused change (or, with `None`, every one the list shows) off the "couldn't sync"
    /// list. One refused since the list opened stays. A cache that can't be written leaves them there,
    /// and the status bar says so.
    fn dismiss_failure(&mut self, id: Option<todo_client::CommandId>, menu: &Menu) {
        let result = match id {
            Some(id) => self.store.dismiss_failed(id).map(drop),
            None => self.store.dismiss_failed_many(&menu.failures).map(drop),
        };
        if let Err(error) = result {
            tracing::warn!(%error, "couldn't dismiss a refused change");
            self.notice = Some(Notice::error(
                "Couldn't dismiss that: this computer's copy couldn't be saved.",
            ));
        }
    }

    /// Moves a list into a collection, or out of any (`None`). Its new collection unfolds, so the list
    /// stays in sight. The toast's Undo moves it back.
    fn move_list(&mut self, id: ListId, to: Option<CollectionId>, cx: &mut Context<Self>) {
        let Some(cmd) = commands::move_list(&self.store.view(), id, to) else {
            return self.refresh(cx);
        };
        if self.submit(vec![cmd]) {
            let revealed = to.is_some_and(|c| self.ui.collapsed_collections.remove(&c));
            return self.refresh_with_collapse_change(revealed, cx);
        }
        self.refresh(cx);
    }

    /// The tag picker's Create: a tag named as typed, put on the task. The picker stays open, its filter
    /// cleared and the keyboard on the new tag. A name that can't be used says why in the status bar.
    fn create_tag(&mut self, task: TaskId, menu: &Menu, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let name = labels::one_line(menu.filter.trim());
        match commands::tag_task_named(&store.view(), task, &name, || store.new_uuid()) {
            Ok((tag, cmds)) => {
                if self.submit(cmds) {
                    if let Some(m) = self.menu.as_mut() {
                        m.filter.clear();
                    }
                    self.menu_scroll = kit::ScrollPane::default();
                    let entries = self.menu_entries(menu.kind);
                    self.highlight(menus::index_of(&entries, &format!("tag-{tag}")));
                }
            }
            Err(e) => self.notice = Some(Notice::error(e.create_message())),
        }
        self.refresh(cx);
    }

    /// Renames a list in place in the sidebar, unfolding its collection first so the editor shows.
    fn rename_list(&mut self, id: ListId, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(list) = state.lists.get(&id) else {
            return;
        };
        let name = SharedString::from(labels::one_line(&list.name));
        let revealed = list
            .collection
            .is_some_and(|c| self.ui.collapsed_collections.remove(&c));
        if revealed {
            self.refresh_with_collapse_change(true, cx);
        }
        self.start_inline(InlineTarget::List(id), name, window, cx);
    }

    /// Opens the paired server's admin pages in the browser. The address is the one learnt at pairing,
    /// never one built into the app.
    fn open_admin(&mut self, cx: &mut Context<Self>) {
        match self.store.server().as_deref().and_then(menus::admin_url) {
            Some(url) => cx.open_url(&url),
            None => {
                self.notice = Some(Notice::info(
                    "The admin pages open once paired with a server.",
                ));
            }
        }
        cx.notify();
    }

    /// Retries syncing now instead of waiting out the backoff.
    fn sync_now(&mut self, cx: &mut Context<Self>) {
        self.notice = Some(match self.store.session() {
            Some(session) => {
                session.client().nudge();
                Notice::info("Syncing now.")
            }
            None => Notice::info("The sample data doesn't sync."),
        });
        cx.notify();
    }

    /// Keep a detail field visible while editing: hang its picker on the roomier side of the row.
    fn menu_opens_up(&self, kind: MenuKind) -> bool {
        let probe = match kind {
            MenuKind::Date(c) if c.task.is_some() => {
                Some(if c.starts { "starts-row" } else { "due-row" })
            }
            MenuKind::Status {
                task: Some(_),
                at_row: false,
            } => Some("status-row"),
            MenuKind::Repeat { .. } => Some("repeat-row"),
            MenuKind::Reminder { .. } => Some("reminder-row"),
            _ => None,
        };
        if let Some(probe) = probe
            && let Some(row) = self.probes.borrow().get(probe)
        {
            let pane = self.detail_scroll.bounds();
            return row.top() - pane.top() > pane.bottom() - row.bottom();
        }
        kind.opens_up()
    }

    /// The open menu, if `belongs` says it is drawn here: at the pointer (`at`), or inside its button.
    pub(super) fn popover(
        &self,
        belongs: impl Fn(&MenuKind) -> bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref().filter(|m| belongs(&m.kind))?;
        let updates = (menu.kind == MenuKind::Updates).then(|| self.update_snapshot());
        let entries = updates
            .as_ref()
            .map_or_else(|| self.menu_entries(menu.kind), Self::update_entries);
        if entries.is_empty() && !matches!(menu.kind, MenuKind::Date(_)) {
            return None;
        }
        let p = self.palette;
        let look = menu.overlay.look(self.frame);
        let live = look.open;
        let kind = menu.kind;
        let footer_menu = match kind {
            MenuKind::BulkMove => Some("Move selected tasks to…"),
            MenuKind::BulkStatus => Some("Set status for selected tasks"),
            MenuKind::New => Some("New"),
            MenuKind::Account { confirm: false } => Some("Settings"),
            MenuKind::Updates => Some("Updates"),
            MenuKind::DateFormat => Some("Date format"),
            MenuKind::QuickPriority => Some("Choose priority"),
            MenuKind::Status { .. } => Some("Choose status"),
            _ => None,
        };
        let on = |item: &MenuItem| menu.highlight.as_ref() == Some(&item.key);
        let rows: Vec<AnyElement> = menus::scrolling(&entries)
            .map(|entry| match entry {
                MenuEntry::Item(item)
                    if kind == MenuKind::Updates
                        && matches!(
                            item.action,
                            MenuAction::CheckUpdates | MenuAction::RestartToUpdate
                        ) =>
                {
                    div()
                        .px(px(space::S3))
                        .pb(px(space::S2))
                        .child(self.confirm_button(&p, item, on(item), live, cx))
                        .into_any_element()
                }
                MenuEntry::Item(item) if applies(item.action) => div()
                    .px(px(space::S2))
                    .pb(px(space::S2))
                    .child(self.confirm_button(&p, item, on(item), live, cx))
                    .into_any_element(),
                MenuEntry::Item(item) => self
                    .menu_item(&p, item, on(item), live, cx)
                    .when(live && footer_menu.is_some(), |d| {
                        d.role(Role::MenuItem)
                            .aria_label(item.label.clone())
                            .when(on(item), |d| d.aria_active_descendant())
                    })
                    .into_any_element(),
                MenuEntry::Confirm {
                    message,
                    note,
                    buttons,
                } => {
                    let buttons = buttons
                        .iter()
                        .map(|item| self.confirm_button(&p, item, on(item), live, cx));
                    div()
                        .px(px(space::S3))
                        .pt(px(space::S2))
                        .pb(px(space::S1))
                        .flex()
                        .flex_col()
                        .gap(px(space::S3))
                        // The consequence in full: it wraps, so the name and the count are never cut.
                        .child(
                            div()
                                .style(ty::BODY)
                                .text_color(p.ink)
                                .child(message.clone()),
                        )
                        // Asked again: it changed while it was open.
                        .children(
                            note.clone().map(|note| {
                                div().style(ty::META).text_color(p.danger_ink).child(note)
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(space::S2))
                                .children(buttons),
                        )
                        .into_any_element()
                }
                // A refused change: what it was and why, both in full (they wrap), beside Dismiss.
                MenuEntry::Failure { what, why, dismiss } => {
                    let highlighted = on(dismiss);
                    div()
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(component::META_GAP))
                                .child(
                                    div()
                                        .style(ty::NAV_SUB)
                                        .text_color(p.ink)
                                        .child(what.clone()),
                                )
                                .child(
                                    div().style(ty::META).text_color(p.ink_3).child(why.clone()),
                                ),
                        )
                        .child(self.confirm_button(&p, dismiss, highlighted, live, cx))
                        .into_any_element()
                }
                MenuEntry::Separator => div()
                    .h(px(elevation::HAIRLINE))
                    .bg(p.line)
                    .my(px(space::S1))
                    .into_any_element(),
                MenuEntry::Caption { label, value } => div()
                    .px(px(space::S3))
                    .pt(px(space::S2))
                    .pb(px(space::S2))
                    .child(kit::label(&p, label))
                    .child(
                        div()
                            .pt(px(space::S1))
                            .truncate()
                            .style(ty::TAG)
                            .text_color(p.ink_2)
                            .child(value.clone()),
                    )
                    .into_any_element(),
                MenuEntry::Heading(text) => div()
                    .px(px(space::S3))
                    .pt(px(space::S2))
                    .pb(px(space::S1))
                    .truncate()
                    .text_color(p.ink_3)
                    .style(ty::CAPS_LABEL)
                    .child(text.to_uppercase())
                    .into_any_element(),
                MenuEntry::SettingToggle { item, description } => self
                    .setting_toggle(item, description, on(item), live, cx)
                    .into_any_element(),
                MenuEntry::DefaultView(items) => self
                    .default_view_choices(items, on, live, cx)
                    .into_any_element(),
                MenuEntry::Note(text) if kind.settings() => div()
                    .mx(px(space::S2))
                    .my(px(space::S1))
                    .p(px(space::S2))
                    .rounded(px(radius::MD))
                    .bg(p.canvas)
                    .style(ty::META)
                    .text_color(p.ink_3)
                    .child(text.clone())
                    .into_any_element(),
                MenuEntry::Note(text) => div()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .style(ty::BODY)
                    .text_color(p.ink_3)
                    .child(text.clone())
                    .into_any_element(),
                MenuEntry::Controls {
                    label,
                    value,
                    buttons,
                } => {
                    let mut row = div().flex().items_center().gap(px(space::S1));
                    // A stepper's value sits between its halves: − buttons, the value, + buttons.
                    let middle = buttons.len() / 2;
                    // With two steps a side, the buttons keep a fixed width and the value takes
                    // the rest, so a day's name fits.
                    let compact = value.is_some() && buttons.len() > 2;
                    for (i, b) in buttons.iter().enumerate() {
                        if i == middle
                            && let Some(n) = value
                        {
                            row = row.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .text_center()
                                    .style(ty::DUE)
                                    .text_color(p.ink)
                                    .child(n.clone()),
                            );
                        }
                        let action = b.action;
                        row = row.child(
                            div()
                                .id(b.key.clone())
                                .when(compact, |d| {
                                    d.flex_none().w(px(desktop::NAV_ROW + space::S2))
                                })
                                .when(!compact, |d| d.flex_1().min_w_0())
                                .h(px(desktop::NAV_ROW))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(radius::BUTTON))
                                .border(px(elevation::HAIRLINE))
                                .border_color(if on(b) { p.accent } else { p.line_strong })
                                .bg(if b.checked { p.tint } else { p.pane })
                                .text_color(if b.checked { p.accent_strong } else { p.ink })
                                .style(ty::BUTTON_SMALL)
                                .child(b.label.clone())
                                .when(live, |d| {
                                    d.cursor_pointer().hover(|s| s.bg(p.hover)).on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.run_menu_action(action, false, window, cx)
                                        }),
                                    )
                                }),
                        );
                    }
                    div()
                        .px(px(space::S2))
                        .py(px(space::S1))
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(kit::label(&p, label))
                        .child(row)
                        .into_any_element()
                }
                MenuEntry::Swatches(items) => div()
                    .px(px(space::S2))
                    .pt(px(space::S1))
                    .pb(px(space::S2))
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S1))
                    .children(
                        items
                            .iter()
                            .map(|item| self.swatch(&p, item, on(item), live, cx)),
                    )
                    .into_any_element(),
                MenuEntry::Filter { .. } => div().into_any_element(),
            })
            .collect();
        // The list picker's filter line stays above the lists, which scroll.
        let filter = entries.iter().find_map(|e| match e {
            MenuEntry::Filter { typed, hint } => Some(
                self.filter_line(&p, typed.clone(), hint)
                    .opacity(look.content),
            ),
            _ => None,
        });
        // It moves the last 4 px away from its anchor as it enters.
        let shift = look.shift * space::S1;
        let opens_up = self.menu_opens_up(menu.kind);
        let shift = if opens_up { shift } else { -shift };
        let width = if kind == MenuKind::New {
            component::NEW_MENU
        } else if kind.settings() || matches!(kind, MenuKind::Updates | MenuKind::DateFormat) {
            desktop::CONFIRM + space::S4
        } else if matches!(kind, MenuKind::Date(_)) || menus::is_wide(&entries) {
            desktop::CONFIRM
        } else {
            desktop::MENU
        };
        // A long menu (the list picker, many refused changes) scrolls past `MENU_ROWS` rows.
        // Rows already inset their content by `space::S1 + space::S2` (12 px >= SCROLLBAR_GUTTER).
        let items = self
            .menu_scroll
            .viewport("menu-items")
            .opacity(look.content)
            .flex()
            .flex_col()
            .max_h(px(desktop::NAV_ROW * MENU_ROWS))
            .children(rows);
        let contents = match kind {
            MenuKind::Date(calendar) => self.calendar_body(calendar, live, cx).into_any_element(),
            _ => div()
                .relative()
                .child(items)
                .child(
                    self.menu_scroll
                        .bar
                        .overlay(&self.menu_scroll.handle, &p, self.animate),
                )
                .into_any_element(),
        };
        // A closing menu is paint-only: it neither blocks nor takes the clicks meant for what's behind.
        let body = kit::overlay(&p)
            .id("menu")
            .w(px(width))
            .relative()
            .top(px(shift))
            .opacity(look.alpha)
            .when(
                matches!(
                    kind,
                    MenuKind::Task {
                        page: TaskPage::PickMerge,
                        ..
                    }
                ),
                |d| {
                    d.child(
                        div()
                            .px(px(space::S3))
                            .pt(px(space::S2))
                            .opacity(look.content)
                            .child(kit::label(&p, "Merge into…")),
                    )
                },
            )
            .children(filter)
            .when_some(updates, |body, snapshot| {
                body.child(self.update_header(&snapshot).opacity(look.content))
                    .children(
                        self.update_busy_button(&snapshot.state)
                            .map(|button| button.opacity(look.content)),
                    )
            })
            .child(contents)
            .when(live, |el| match kind {
                MenuKind::Date(c) => el.role(Role::Dialog).aria_label(if c.starts {
                    "Choose start date"
                } else {
                    "Choose due date"
                }),
                kind if kind.settings() => el.role(Role::Dialog).aria_label("List settings"),
                _ => el,
            })
            .when_some(footer_menu.filter(|_| live), |d, label| {
                d.role(Role::Menu).aria_label(label)
            })
            .when(live, |el| {
                el.occlude()
                    .track_focus(&self.menu_focus)
                    .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        this.menu_key(e, window, cx);
                    }))
                    // The press that closes the menu is remembered, so its click on the menu's own
                    // button doesn't open it again (`toggle_menu`).
                    .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                        this.menu_press.closed(kind);
                        this.close_menu(false, window, cx);
                    }))
            });
        let popover = match menu.at {
            Some(at) => anchored()
                .anchor(
                    if matches!(kind, MenuKind::BulkMove | MenuKind::BulkStatus) {
                        Anchor::BottomLeft
                    } else {
                        Anchor::TopLeft
                    },
                )
                .position(at)
                .snap_to_window_with_margin(px(space::S2))
                .child(body),
            None => anchored()
                .anchor(match (opens_up, menu.kind.aligns_left()) {
                    (true, true) => Anchor::BottomLeft,
                    (true, false) => Anchor::BottomRight,
                    (false, true) => Anchor::TopLeft,
                    (false, false) => Anchor::TopRight,
                })
                .snap_to_window_with_margin(px(space::S2))
                .child(body),
        };
        Some(deferred(popover).with_priority(2).into_any_element())
    }

    /// A picker's filter line: what's been typed, or a hint that typing narrows it.
    fn filter_line(&self, p: &Palette, typed: SharedString, hint: &'static str) -> Div {
        let empty = typed.is_empty();
        div()
            .mx(px(space::S1))
            .mb(px(space::S1))
            .h(px(desktop::NAV_ROW))
            .px(px(space::S2))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .border_b(px(elevation::HAIRLINE))
            .border_color(p.line)
            .style(ty::NAV_SUB)
            .child(kit::icon(Icon::Search, component::ICON_SMALL, p.ink_3))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if empty { p.ink_3 } else { p.ink })
                    .child(if empty {
                        SharedString::from(hint)
                    } else {
                        typed
                    }),
            )
    }

    /// A confirmation's scrim, over everything below the title bar (which keeps the window's own
    /// controls). The confirmation is modal: while it's open, the scrim takes every click and hover meant
    /// for what's under it, so a click outside only dismisses it (the menu's own outside-press handler
    /// closes it) and never also completes a task or opens a row. Once it's closing, the scrim is
    /// paint-only and fades out with the card.
    pub(super) fn confirmation_scrim(&self) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let look = menu.overlay.look(self.frame);
        let confirmation = menus::is_confirmation(&self.menu_entries(menu.kind));
        let scrim = Scrim::of(confirmation, look.open);
        if scrim == Scrim::None {
            return None;
        }
        let layer = div()
            .id(if scrim == Scrim::Blocking {
                "confirm-scrim"
            } else {
                "confirm-scrim-closing"
            })
            .absolute()
            .top(px(desktop::TITLEBAR))
            .left_0()
            .right_0()
            .bottom_0()
            .bg(self.palette.scrim)
            .opacity(look.alpha)
            .when(scrim == Scrim::Blocking, |d| d.occlude());
        Some(deferred(layer).with_priority(1).into_any_element())
    }

    /// Where a button's menu hangs from: its bottom-right corner (or top-right, for a menu that opens
    /// upwards), a small gap away.
    pub(super) fn menu_anchor(
        &self,
        belongs: impl Fn(&MenuKind) -> bool,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let opens_up = self
            .menu
            .as_ref()
            .is_some_and(|m| self.menu_opens_up(m.kind));
        let left = self.menu.as_ref().is_some_and(|m| m.kind.aligns_left());
        let popover = self.popover(belongs, cx)?;
        let gap = px(space::S1);
        Some(
            div()
                .absolute()
                .when(left, |d| d.left_0())
                .when(!left, |d| d.right_0())
                .when(opens_up, |d| d.top(-gap))
                .when(!opens_up, |d| d.bottom(-gap))
                .size_0()
                .child(popover),
        )
    }

    fn menu_item(
        &self,
        p: &Palette,
        item: &MenuItem,
        highlighted: bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let ink = if item.danger {
            p.danger_ink
        } else if item.checked {
            p.accent_strong
        } else {
            p.ink
        };
        let action = item.action;
        let hover = if item.checked { p.tint } else { p.hover };
        let merge_choice = matches!(action, MenuAction::AskMerge(..));
        let tag_choice = matches!(action, MenuAction::ToggleTag(..)) && item.more.is_some();
        let identity = item.dot.map(|hue| theme::list_colors(self.theme, hue));
        div()
            .id(item.key.clone())
            .group(item.key.clone())
            .mx(px(space::S1))
            .h(px(
                desktop::NAV_ROW + if merge_choice { space::S3 } else { 0. }
            ))
            .px(px(space::S2))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .border_color(if highlighted {
                p.accent_strong
            } else {
                gpui::transparent_black()
            })
            .when(item.checked, |d| d.bg(p.wash))
            .when(highlighted, |d| d.bg(hover))
            .style(ty::NAV_SUB)
            .when(item.checked, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
            .text_color(ink)
            .children((!tag_choice).then(|| match item.glyph {
                Some((glyph, hue)) => kit::glyph(glyph, theme::list_colors(self.theme, hue).glyph),
                None => kit::icon(
                    item.icon,
                    component::ICON_SMALL,
                    if item.danger {
                        p.danger
                    } else if let Some(colors) = &identity {
                        colors.glyph
                    } else {
                        kit::task_toggle_fill(p, item.icon).unwrap_or(p.ink_3)
                    },
                ),
            }))
            .child(if merge_choice {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(item.label.clone()))
                    .children(item.hint.clone().map(|hint| {
                        div()
                            .style(ty::META)
                            .text_color(p.ink_3)
                            .truncate()
                            .child(hint)
                    }))
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(tag_choice, |d| d.style(ty::TAG))
                    .when_some(identity, |d, colors| d.text_color(colors.ink))
                    .child(item.label.clone())
            })
            .children(item.hint.clone().filter(|_| !merge_choice).map(|hint| {
                div()
                    .flex_none()
                    .style(ty::PRIORITY)
                    .text_color(p.ink_3)
                    .child(hint)
            }))
            .child(
                div()
                    .flex_none()
                    .size(px(component::ICON_SMALL))
                    .when(item.checked, |d| {
                        d.child(kit::icon(
                            Icon::Check,
                            component::ICON_SMALL,
                            p.accent_strong,
                        ))
                    }),
            )
            // A second action behind a chevron (Right from the keyboard): a tag's own page.
            .children(item.more.map(|more| {
                let wash = p.wash;
                div()
                    .id(SharedString::from(format!("{}-more", item.key)))
                    // Shown on the row under the pointer or the keyboard, so the list stays calm.
                    .opacity(if highlighted { 1. } else { 0. })
                    .group_hover(item.key.clone(), |s| s.opacity(1.))
                    .flex_none()
                    .size(px(component::ICON_BUTTON - space::S2))
                    .mr(px(-space::S1))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .child(kit::icon(
                        Icon::ChevronRight,
                        component::ICON_SMALL,
                        p.ink_3,
                    ))
                    .when(live, |d| {
                        d.hover(move |s| s.bg(wash)).on_click(cx.listener(
                            move |this, e: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                let keyboard = e.mouse_position().is_none();
                                this.run_menu_action(more, keyboard, window, cx);
                            },
                        ))
                    })
            }))
            .when(live, |d| {
                d.hover(move |s| s.bg(hover)).on_click(cx.listener(
                    move |this, e: &gpui::ClickEvent, window, cx| {
                        let keyboard = e.mouse_position().is_none();
                        this.run_menu_action(action, keyboard, window, cx);
                    },
                ))
            })
    }
}

impl ShellView {
    /// A colour swatch: a disc in its colour, ticked when it's the current one. The keyboard's choice
    /// wears the focus ring; hovering lifts it into a ring of its own colour.
    fn swatch(
        &self,
        p: &Palette,
        item: &MenuItem,
        highlighted: bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let action = item.action;
        let colors = theme::list_colors(self.theme, item.dot.unwrap_or_default());
        let size = component::ICON_BUTTON;
        let disc = component::ICON + space::S1;
        let tint = colors.tint;
        div()
            .id(item.key.clone())
            .flex_none()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border(px(elevation::HAIRLINE))
            .border_color(if highlighted {
                p.accent
            } else if item.checked {
                colors.swatch
            } else {
                gpui::transparent_black()
            })
            .when(highlighted, |d| d.bg(p.tint))
            .child(
                div()
                    .size(px(disc))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(colors.swatch)
                    .when(item.checked, |d| {
                        d.child(kit::icon(Icon::Check, component::ICON_SMALL, p.on_accent))
                    }),
            )
            .when(live, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(tint))
                    .tooltip(kit::tooltip(*p, item.label.clone()))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        let keyboard = e.mouse_position().is_none();
                        this.run_menu_action(action, keyboard, window, cx);
                    }))
            })
    }

    /// One of a confirmation's short buttons: the destructive action filled in `danger`, Cancel quiet.
    /// The keyboard's choice wears the focus ring.
    fn confirm_button(
        &self,
        p: &Palette,
        item: &MenuItem,
        highlighted: bool,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let action = item.action;
        let small = |b: Div| {
            b.h(px(component::BUTTON_SMALL))
                .px(px(space::S3))
                .style(ty::BUTTON_SMALL)
        };
        // Both keep a hairline border, so the ring's accent border never changes their size.
        let (button, edge) = if item.danger {
            (
                small(kit::button(p, item.label.clone())).bg(p.danger),
                p.danger,
            )
        } else if applies(item.action) || matches!(item.action, MenuAction::Merge(..)) {
            (small(kit::button(p, item.label.clone())), p.accent_fill)
        } else {
            (
                small(kit::button_quiet(p, item.label.clone())),
                p.line_strong,
            )
        };
        let button = button
            .border(px(elevation::HAIRLINE))
            .border_color(if highlighted { p.accent } else { edge })
            .id(item.key.clone())
            .when(
                live && matches!(
                    action,
                    MenuAction::CheckUpdates | MenuAction::RestartToUpdate
                ),
                |d| {
                    d.role(Role::MenuItem)
                        .aria_label(item.label.clone())
                        .when(highlighted, |d| d.aria_active_descendant())
                },
            )
            .when(live, |d| {
                d.cursor_pointer().on_click(cx.listener(
                    move |this, e: &gpui::ClickEvent, window, cx| {
                        let keyboard = e.mouse_position().is_none();
                        this.run_menu_action(action, keyboard, window, cx);
                    },
                ))
            });
        div()
            .flex_none()
            .p(px(elevation::FOCUS_RING))
            .m(px(-elevation::FOCUS_RING))
            .rounded(px(radius::BUTTON + elevation::FOCUS_RING))
            .when(highlighted, |d| d.bg(p.tint))
            .child(button)
    }
}

/// Whether an item applies a picker's draft (Apply repeat, Set reminder): drawn as a filled button.
fn applies(action: MenuAction) -> bool {
    matches!(
        action,
        MenuAction::Repeat(_, repeat::Action::Apply)
            | MenuAction::Reminder(_, reminder::Action::Apply)
            | MenuAction::RestartToUpdate
    )
}

/// Whether the keyboard goes back to the task list after a menu was `dropped` because its subject went
/// away (a task or list deleted on another device): when the keyboard was on the menu, or, the menu's
/// element gone, on nothing at all. Otherwise Escape and Tab would have nowhere to go. A keyboard that
/// has moved on (a text field) is left there.
pub(super) fn keyboard_back_to_list(
    dropped: bool,
    menu_focused: bool,
    nothing_focused: bool,
) -> bool {
    dropped && (menu_focused || nothing_focused)
}

/// What a key does to a picker's filter (the list picker's Move to…, the tag picker): the filter it
/// leaves, or `None` when the key isn't typing there. Letters add to it, Backspace takes the last one back, and Escape clears it before
/// it closes the picker. Space types once something is typed (before that it chooses, as in any menu).
/// A held key's repeats type nothing (only Backspace repeats), so holding M, which opens the picker,
/// doesn't fill it with m's.
fn picker_edit(menu: &Menu, e: &KeyDownEvent) -> Option<String> {
    let m = &e.keystroke.modifiers;
    if !menu.kind.filters() || m.control || m.alt || m.platform || m.function {
        return None;
    }
    if e.is_held && e.keystroke.key != "backspace" {
        return None;
    }
    let typed = &menu.filter;
    match e.keystroke.key.as_str() {
        "backspace" if !typed.is_empty() => {
            let mut typed = typed.clone();
            typed.pop();
            Some(typed)
        }
        "escape" if !typed.is_empty() => Some(String::new()),
        "enter" | "tab" | "escape" | "backspace" => None,
        _ => {
            let c = e.keystroke.key_char.as_deref()?;
            let blank = c.trim().is_empty();
            if c.is_empty() || c.chars().any(char::is_control) || (blank && typed.is_empty()) {
                return None;
            }
            Some(format!("{typed}{c}"))
        }
    }
}

/// Which mouse press closed a menu, so that the click that very press ends on the menu's own button
/// leaves it closed. Every press in the window counts (`press`, from a window-level capture listener
/// that no occluding pane can hide); a menu that a press closes records that press (`closed`). Only
/// the click of the same press is kept from opening that menu again: a later press, wherever it lands,
/// leaves the menu's button working.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PressGuard {
    /// Mouse presses so far.
    presses: u64,
    /// The menu the press `.1` closed.
    closed: Option<(MenuKind, u64)>,
}

impl PressGuard {
    /// A mouse press anywhere in the window, before any menu sees it.
    pub(super) fn press(&mut self) {
        self.presses = self.presses.wrapping_add(1);
    }

    /// The current press closed `kind`.
    pub(super) fn closed(&mut self, kind: MenuKind) {
        self.closed = Some((kind, self.presses));
    }

    /// Whether a mouse click on `kind`'s button ends the press that closed it, so it must not open it
    /// again. It answers once.
    pub(super) fn swallows(&mut self, kind: MenuKind) -> bool {
        let presses = self.presses;
        self.closed
            .take()
            .is_some_and(|(k, at)| at == presses && k.same_menu(kind))
    }
}

/// How a menu's scrim behaves: none for a plain menu; a confirmation is modal, so while it's open its
/// scrim takes every click and hover meant for what's under it (the click only dismisses it), and once
/// it's closing the scrim is paint-only, fading out without taking anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scrim {
    None,
    Blocking,
    PaintOnly,
}

impl Scrim {
    pub(super) fn of(confirmation: bool, open: bool) -> Self {
        match (confirmation, open) {
            (false, _) => Self::None,
            (true, true) => Self::Blocking,
            (true, false) => Self::PaintOnly,
        }
    }
}

/// The focus ring around a small button, kept in the layout when off so focusing never moves it: the
/// 3 px `tint` band and an `accent` border, as on a focused field.
pub(super) fn ring<E: Styled + IntoElement>(
    p: &Palette,
    focused: bool,
    radius: f32,
    button: E,
) -> Div {
    div()
        .flex_none()
        .p(px(elevation::FOCUS_RING))
        .m(px(-elevation::FOCUS_RING))
        .rounded(px(radius + elevation::FOCUS_RING))
        .bg(if focused {
            p.tint
        } else {
            gpui::transparent_black()
        })
        .child(
            button
                .rounded(px(radius))
                .border(px(elevation::HAIRLINE))
                .border_color(if focused {
                    p.accent
                } else {
                    gpui::transparent_black()
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicking_an_open_menus_button_closes_it_and_leaves_it_closed() {
        let room = MenuKind::Room { page: Page::Main };
        let mut guard = PressGuard::default();
        // The press on the button lands outside the open menu, which closes: its click must not reopen it.
        guard.press();
        guard.closed(room);
        assert!(guard.swallows(room));
        // The next click on the button opens it again.
        guard.press();
        assert!(!guard.swallows(room));
        // A confirmation is the same menu.
        guard.press();
        guard.closed(MenuKind::Room {
            page: Page::Confirm,
        });
        assert!(guard.swallows(room));
    }

    #[test]
    fn after_an_outside_dismissal_the_next_click_on_the_button_opens_the_menu() {
        let priority = MenuKind::Priority {
            task: TaskId::from_uuid(uuid::Uuid::nil()),
        };
        let mut guard = PressGuard::default();
        // A press on a blank part of the (occluding) detail pane closes the picker. Its click reaches no
        // button, so nothing reads the guard.
        guard.press();
        guard.closed(priority);
        // The next press, on Priority, is a new press: its click opens the picker.
        guard.press();
        assert!(!guard.swallows(priority));
        // Again, with a second dismissal before the click: only the closing press itself counts.
        guard.press();
        guard.closed(priority);
        guard.press();
        assert!(!guard.swallows(priority));
    }

    #[test]
    fn a_press_that_closed_another_menu_doesnt_stop_this_one_opening() {
        let list = ListId::from_uuid(uuid::Uuid::nil());
        let room = MenuKind::Room { page: Page::Main };
        let mut guard = PressGuard::default();
        guard.press();
        guard.closed(MenuKind::Account { confirm: false });
        assert!(!guard.swallows(room));
        guard.press();
        guard.closed(MenuKind::List {
            id: list,
            page: Page::Main,
        });
        assert!(!guard.swallows(room));
        // Nothing closed: a click opens it.
        assert!(!PressGuard::default().swallows(room));
    }

    #[test]
    fn typing_in_the_list_picker_narrows_it_and_escape_clears_it_first() {
        use gpui::{KeyDownEvent, Keystroke};
        let task = TaskId::from_uuid(uuid::Uuid::nil());
        let menu = |page, filter: &str| Menu {
            kind: MenuKind::Task { id: task, page },
            at: None,
            highlight: None,
            overlay: Overlay::rest(),
            trigger: None,
            filter: filter.into(),
            failures: Vec::new(),
            asked: None,
            move_from: None,
        };
        let key = |text: &'static str| KeyDownEvent {
            keystroke: Keystroke::parse(text).expect("keystroke"),
            is_held: false,
            prefer_character_input: false,
        };
        let held = |text: &'static str| KeyDownEvent {
            is_held: true,
            ..key(text)
        };
        let typed = |m: &Menu, k: &'static str| picker_edit(m, &key(k));
        let empty = menu(TaskPage::Move, "");
        // As Windows reports them: the key, and the character it types.
        assert_eq!(typed(&empty, "e->e").as_deref(), Some("e"));
        assert_eq!(typed(&empty, "shift-e->E").as_deref(), Some("E"));
        // Space chooses until something is typed, then it types.
        assert_eq!(typed(&empty, "space-> "), None);
        let some = menu(TaskPage::Move, "err");
        assert_eq!(typed(&some, "space-> ").as_deref(), Some("err "));
        assert_eq!(typed(&some, "backspace").as_deref(), Some("er"));
        // Holding M (which opened it) types nothing more; holding Backspace keeps deleting.
        assert_eq!(picker_edit(&empty, &held("m->m")), None);
        assert_eq!(picker_edit(&some, &held("m->m")), None);
        assert_eq!(
            picker_edit(&some, &held("backspace")).as_deref(),
            Some("er")
        );
        assert_eq!(typed(&some, "escape").as_deref(), Some(""));
        // Escape and Backspace with nothing typed, and the keys that walk and choose, are the menu's.
        for k in ["escape", "backspace", "enter", "tab", "down", "up", "home"] {
            assert_eq!(typed(&empty, k), None, "{k}");
        }
        assert_eq!(typed(&some, "enter"), None);
        assert_eq!(typed(&some, "ctrl-a->a"), None);
        // Only the list picker takes typing.
        assert_eq!(typed(&menu(TaskPage::Main, ""), "e->e"), None);
        assert_eq!(typed(&menu(TaskPage::Priority, ""), "e->e"), None);
    }

    #[test]
    fn a_menu_whose_subject_went_away_gives_the_keyboard_back_to_the_list() {
        // On the menu, or nowhere once its element stopped rendering: back to the list.
        assert!(keyboard_back_to_list(true, true, false));
        assert!(keyboard_back_to_list(true, false, true));
        // Typing somewhere else meanwhile: left there.
        assert!(!keyboard_back_to_list(true, false, false));
        // Nothing was dropped: the usual rules apply.
        assert!(!keyboard_back_to_list(false, true, false));
    }

    #[test]
    fn a_tasks_menu_and_its_pickers_are_one_menu() {
        let a = TaskId::from_uuid(uuid::Uuid::nil());
        let b = TaskId::from_uuid(uuid::Uuid::max());
        let main = MenuKind::Task {
            id: a,
            page: TaskPage::Main,
        };
        let picker = MenuKind::Task {
            id: a,
            page: TaskPage::Move,
        };
        assert!(main.same_menu(picker));
        assert!(!main.same_menu(MenuKind::Task {
            id: b,
            page: TaskPage::Main
        }));
        assert_eq!(picker.task(), Some(a));
        assert_eq!(MenuKind::SyncFailures.task(), None);
        // The sync line's list opens upwards from the left edge of the sidebar.
        assert!(MenuKind::SyncFailures.opens_up() && MenuKind::SyncFailures.aligns_left());
        assert!(!main.opens_up() && !main.aligns_left());
    }

    #[test]
    fn a_menus_pages_are_one_menu_and_only_the_pickers_take_typing() {
        let list = ListId::from_uuid(uuid::Uuid::nil());
        let col = CollectionId::from_uuid(uuid::Uuid::nil());
        let other = CollectionId::from_uuid(uuid::Uuid::max());
        let task = TaskId::from_uuid(uuid::Uuid::nil());
        let tag = todo_core::ids::TagId::from_uuid(uuid::Uuid::nil());
        let main = MenuKind::List {
            id: list,
            page: Page::Main,
        };
        // Asking for a page turns the same menu into it.
        for page in [Page::Confirm, Page::Move, Page::Colour] {
            assert_eq!(main.on_page(page), MenuKind::List { id: list, page });
            assert!(main.same_menu(main.on_page(page)));
            assert_eq!(
                MenuKind::Room { page: Page::Main }.on_page(page),
                MenuKind::Room { page }
            );
        }
        let collection = MenuKind::Collection {
            id: col,
            page: Page::Main,
        };
        assert!(collection.same_menu(collection.on_page(Page::Colour)));
        assert!(!collection.same_menu(MenuKind::Collection {
            id: other,
            page: Page::Main
        }));
        let pick = MenuKind::Tags {
            task,
            page: TagPage::Pick,
        };
        let edit = MenuKind::Tags {
            task,
            page: TagPage::Edit(tag),
        };
        assert!(pick.same_menu(edit));
        // What's typed goes to the list picker and the tag picker, nowhere else.
        assert!(pick.filters());
        assert!(
            MenuKind::Task {
                id: task,
                page: TaskPage::Move
            }
            .filters()
        );
        for kind in [
            edit,
            main,
            collection,
            MenuKind::Task {
                id: task,
                page: TaskPage::Links,
            },
        ] {
            assert!(!kind.filters(), "{kind:?}");
        }
        let typing = Menu {
            kind: pick,
            at: None,
            highlight: None,
            overlay: Overlay::rest(),
            trigger: None,
            filter: "rec".into(),
            failures: Vec::new(),
            asked: None,
            move_from: None,
        };
        let key = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("e->e").expect("keystroke"),
            is_held: false,
            prefer_character_input: false,
        };
        assert_eq!(picker_edit(&typing, &key).as_deref(), Some("rece"));
        let on_edit = Menu {
            kind: edit,
            ..typing
        };
        assert_eq!(picker_edit(&on_edit, &key), None);
    }

    #[test]
    fn a_confirmation_is_modal_until_it_starts_closing() {
        // A plain menu has no scrim.
        assert_eq!(Scrim::of(false, true), Scrim::None);
        assert_eq!(Scrim::of(false, false), Scrim::None);
        // An open confirmation's scrim takes the click, so a checkbox under it is never completed.
        assert_eq!(Scrim::of(true, true), Scrim::Blocking);
        // Once it's closing, the scrim only paints its fade.
        assert_eq!(Scrim::of(true, false), Scrim::PaintOnly);
    }

    #[test]
    fn a_confirmation_is_the_same_menu() {
        let list = ListId::from_uuid(uuid::Uuid::nil());
        assert!(
            MenuKind::Room { page: Page::Main }.same_menu(MenuKind::Room {
                page: Page::Confirm
            })
        );
        assert!(
            MenuKind::Account { confirm: false }.same_menu(MenuKind::Account { confirm: true })
        );
        assert!(
            MenuKind::List {
                id: list,
                page: Page::Main
            }
            .same_menu(MenuKind::List {
                id: list,
                page: Page::Confirm
            })
        );
        assert!(
            !MenuKind::Room { page: Page::Main }.same_menu(MenuKind::Account { confirm: false })
        );
        // Only the account menu opens upwards (from the sidebar footer).
        assert!(MenuKind::Account { confirm: false }.opens_up());
        assert!(!MenuKind::Room { page: Page::Main }.opens_up());
    }
}
