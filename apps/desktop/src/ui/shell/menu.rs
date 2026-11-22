//! The popover menus: a list's or a collection's context menu (at the pointer), and the menus that open
//! from a button (the room header's "more", the account gear, the detail pane's priority row).
//!
//! What a menu offers is data (`model::menus`); this draws it and runs the chosen item. Every menu
//! takes the keyboard while it is open: Up and Down (Left and Right between a confirmation's buttons)
//! walk its items, Enter or Space choose, Escape or Tab close it and give the keyboard back to its
//! button (or the list). It closes whenever the keyboard goes anywhere else (a shortcut, a click into a
//! field, another window). It enters over `OVERLAY_IN` and fades out over `OVERLAY_OUT`
//! (`model::motion::Overlay`); a closing menu is paint-only: it takes no clicks, keys or hovers.

use gpui::{
    Anchor, AnyElement, Context, Div, KeyDownEvent, Pixels, Point, ScrollHandle, SharedString,
    Window, anchored, deferred, div, prelude::*, px,
};
use todo_core::ids::{CollectionId, ListId, TaskId};

use super::sidebar::InlineTarget;
use super::{Notice, ShellEvent, ShellView};
use crate::model::menus::{self, MenuAction, MenuEntry, MenuItem};
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
    /// The item the keyboard is on, if it has moved yet.
    pub highlight: Option<usize>,
    pub overlay: Overlay,
    /// Where closing the menu from the keyboard gives the keyboard back: its button.
    pub trigger: Option<Stop>,
    /// What's been typed to narrow the list picker.
    pub filter: String,
}

/// What a task's context menu shows: its items, or in their place the priority picker or the list
/// picker that one of them opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskPage {
    Main,
    Priority,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuKind {
    /// The sidebar footer's menu: the server, its admin pages, Sync now and Sign out (`confirm` once
    /// asked).
    Account {
        confirm: bool,
    },
    /// A list's context menu (`confirm` once Delete was chosen).
    List {
        id: ListId,
        confirm: bool,
    },
    Collection {
        id: CollectionId,
    },
    /// The room header's "more" menu (`confirm` once Delete list was chosen).
    Room {
        confirm: bool,
    },
    /// The detail pane's priority picker.
    Priority {
        task: TaskId,
    },
    /// A task's context menu (right-click its row, or Shift+F10), or the list picker (M).
    Task {
        id: TaskId,
        page: TaskPage,
    },
    /// The sidebar footer's "N couldn't sync": the refused changes, to see and dismiss.
    SyncFailures,
}

impl MenuKind {
    /// Which way the menu opens from its anchor: down (shifting in from above) or up.
    fn opens_up(self) -> bool {
        matches!(self, Self::Account { .. } | Self::SyncFailures)
    }

    /// Whether a button's menu hangs from its left edge (the sync line, at the sidebar's left) rather
    /// than its right.
    fn aligns_left(self) -> bool {
        matches!(self, Self::SyncFailures)
    }

    /// The same menu, apart from its confirmation step (or the picker a task's menu turned into).
    pub(super) fn same_menu(self, other: Self) -> bool {
        match (self, other) {
            (Self::Account { .. }, Self::Account { .. })
            | (Self::Room { .. }, Self::Room { .. }) => true,
            (Self::List { id: a, .. }, Self::List { id: b, .. }) => a == b,
            (Self::Task { id: a, .. }, Self::Task { id: b, .. }) => a == b,
            _ => self == other,
        }
    }

    /// The task a context menu or picker is about.
    pub(super) fn task(self) -> Option<TaskId> {
        match self {
            Self::Task { id, .. } => Some(id),
            _ => None,
        }
    }
}

impl ShellView {
    /// What the menu offers now.
    pub(super) fn menu_entries(&self, kind: MenuKind) -> Vec<MenuEntry> {
        let state = self.store.view();
        match kind {
            MenuKind::Account { confirm } => {
                menus::account_menu(self.store.server().as_deref(), confirm)
            }
            MenuKind::List { id, confirm } => menus::list_menu(&state, id, confirm),
            MenuKind::Collection { id } => {
                menus::collection_menu(&state, id, self.ui.collapsed_collections.contains(&id))
            }
            MenuKind::Room { confirm } => {
                menus::room_menu(&state, self.shell.room.key, self.ui.show_completed, confirm)
            }
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
            } => menus::task_menu(&state, id, self.store.now().today()),
            MenuKind::Task {
                id,
                page: TaskPage::Move,
            } => {
                let filter = self.menu.as_ref().map_or("", |m| m.filter.as_str());
                menus::move_menu(&state, id, filter)
            }
            MenuKind::SyncFailures => menus::failures_menu(&state, &self.store.failed()),
        }
    }

    /// Whether the room header shows its "more" button: only when its menu offers something.
    pub(super) fn has_room_menu(&self) -> bool {
        !menus::room_menu(
            &self.store.view(),
            self.shell.room.key,
            self.ui.show_completed,
            false,
        )
        .is_empty()
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
        let menu = match open {
            Some(m) => Menu {
                kind,
                filter: String::new(),
                ..m.clone()
            },
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
            },
        };
        // A keyboard user stays on the keyboard: a confirmation starts on Cancel.
        let keyboard = menu.highlight.is_some();
        self.menu = Some(menu);
        // A new menu, or a new page of it, starts scrolled to its top.
        self.menu_scroll = ScrollHandle::new();
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
    fn highlight(&mut self, at: Option<usize>) {
        let Some(kind) = self.menu.as_ref().map(|m| m.kind) else {
            return;
        };
        let child = at.and_then(|i| menus::child_of_item(&self.menu_entries(kind), i));
        if let Some(menu) = self.menu.as_mut() {
            menu.highlight = at;
        }
        if let Some(child) = child {
            self.menu_scroll.scroll_to_item(child);
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
        let entries = self.menu_entries(menu.kind);
        let count = menus::items(&entries).count();
        if let Some(filter) = picker_edit(&menu, e) {
            if let Some(m) = self.menu.as_mut() {
                m.filter = filter;
            }
            // The keyboard goes to the first list that matches, at the top.
            let first = menus::items(&self.menu_entries(menu.kind))
                .next()
                .map(|_| 0);
            self.menu_scroll = ScrollHandle::new();
            self.highlight(first);
            cx.notify();
            return;
        }
        match key {
            "down" | "up" | "right" | "left" => {
                let by = if matches!(key, "down" | "right") {
                    1
                } else {
                    -1
                };
                let next = menus::step_highlight(menu.highlight, by, count);
                self.highlight(next);
                cx.notify();
            }
            "home" | "end" if count > 0 => {
                let at = if key == "home" { 0 } else { count - 1 };
                self.highlight(Some(at));
                cx.notify();
            }
            "enter" | "space" if !e.is_held => {
                let chosen = menu
                    .highlight
                    .and_then(|i| menus::items(&entries).nth(i))
                    .map(|i| i.action);
                if let Some(action) = chosen {
                    self.run_menu_action(action, true, window, cx);
                }
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
        if keyboard && let Some(m) = self.menu.as_mut() {
            m.highlight = Some(m.highlight.unwrap_or(0));
        }
        match action {
            MenuAction::AskDeleteList(id) => {
                let kind = match menu.kind {
                    MenuKind::Room { .. } => MenuKind::Room { confirm: true },
                    _ => MenuKind::List { id, confirm: true },
                };
                return self.open_menu(kind, menu.at, menu.trigger, window, cx);
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
            MenuAction::AskPriority(id) | MenuAction::AskMove(id) => {
                let page = if matches!(action, MenuAction::AskMove(_)) {
                    TaskPage::Move
                } else {
                    TaskPage::Priority
                };
                let kind = MenuKind::Task { id, page };
                self.open_menu(kind, menu.at, menu.trigger, window, cx);
                self.highlight_first();
                return;
            }
            // The list stays open while more are left, the keyboard on the item that took its place.
            MenuAction::DismissFailure(id) => {
                self.dismiss_failure(Some(id));
                let left = menus::items(&self.menu_entries(menu.kind)).count();
                if left > 0 {
                    self.highlight(menu.highlight.map(|i| i.min(left - 1)));
                    self.refresh(cx);
                    return;
                }
            }
            _ => {}
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
            MenuAction::DeleteList(id) => {
                self.submit(vec![commands::delete(id)]);
                self.refresh(cx);
            }
            MenuAction::Nudge(id, by) => self.nudge(id, by, cx),
            MenuAction::ToggleCollection(id) => self.toggle_collection(id, cx),
            MenuAction::NewListIn(id) => self.new_list(Some(id), window, cx),
            MenuAction::OpenAdmin => self.open_admin(cx),
            MenuAction::SyncNow => self.sync_now(cx),
            MenuAction::SignOut => cx.emit(ShellEvent::SignOut),
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
            MenuAction::MoveTask(task, list) => self.move_task_to(task, list, cx),
            MenuAction::DeleteTask(task) => self.delete_task(task, cx),
            MenuAction::DismissAllFailures => {
                self.dismiss_failure(None);
                self.refresh(cx);
            }
            MenuAction::DismissFailure(_) => self.refresh(cx),
            MenuAction::AskDeleteList(_)
            | MenuAction::AskSignOut
            | MenuAction::AskPriority(_)
            | MenuAction::AskMove(_)
            | MenuAction::Cancel => {}
        }
    }

    /// Takes one refused change (or, with `None`, all of them) off the "couldn't sync" list. A cache
    /// that can't be written leaves them there, and the status bar says so.
    fn dismiss_failure(&mut self, id: Option<todo_client::CommandId>) {
        let result = match id {
            Some(id) => self.store.dismiss_failed(id).map(drop),
            None => self.store.dismiss_all_failed().map(drop),
        };
        if let Err(error) = result {
            tracing::warn!(%error, "couldn't dismiss a refused change");
            self.notice = Some(Notice::error(
                "Couldn't dismiss that: this computer's copy couldn't be saved.",
            ));
        }
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

    /// The open menu, if `belongs` says it is drawn here: at the pointer (`at`), or inside its button.
    pub(super) fn popover(
        &self,
        belongs: impl Fn(&MenuKind) -> bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref().filter(|m| belongs(&m.kind))?;
        let entries = self.menu_entries(menu.kind);
        if entries.is_empty() {
            return None;
        }
        let p = self.palette;
        let look = menu.overlay.look(self.frame);
        let live = look.open;
        let kind = menu.kind;
        let mut index = 0;
        let rows: Vec<AnyElement> = menus::scrolling(&entries)
            .map(|entry| match entry {
                MenuEntry::Item(item) => {
                    let i = index;
                    index += 1;
                    self.menu_item(&p, item, menu.highlight == Some(i), live, cx)
                        .into_any_element()
                }
                MenuEntry::Confirm { message, buttons } => {
                    let first = index;
                    index += buttons.len();
                    let buttons = buttons.iter().enumerate().map(|(k, item)| {
                        let highlighted = menu.highlight == Some(first + k);
                        self.confirm_button(&p, item, highlighted, live, cx)
                    });
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
                    let highlighted = menu.highlight == Some(index);
                    index += 1;
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
                                .gap(px(component::CHIP_GAP))
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
                MenuEntry::Note(text) => div()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .style(ty::BODY)
                    .text_color(p.ink_3)
                    .child(text.clone())
                    .into_any_element(),
                MenuEntry::Filter(_) => div().into_any_element(),
            })
            .collect();
        // The list picker's filter line stays above the lists, which scroll.
        let filter = entries.iter().find_map(|e| match e {
            MenuEntry::Filter(typed) => Some(self.filter_line(&p, typed.clone())),
            _ => None,
        });
        // It moves the last 4 px away from its anchor as it enters.
        let shift = look.shift * space::S1;
        let shift = if menu.kind.opens_up() { shift } else { -shift };
        let width = if menus::is_wide(&entries) {
            desktop::CONFIRM
        } else {
            desktop::MENU
        };
        // A long menu (the list picker, many refused changes) scrolls past `MENU_ROWS` rows.
        let items = div()
            .id("menu-items")
            .flex()
            .flex_col()
            .max_h(px(desktop::NAV_ROW * MENU_ROWS))
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .children(rows);
        // A closing menu is paint-only: it neither blocks nor takes the clicks meant for what's behind.
        let body = kit::overlay(&p)
            .id("menu")
            .w(px(width))
            .relative()
            .top(px(shift))
            .opacity(look.alpha)
            .children(filter)
            .child(items)
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
                .position(at)
                .snap_to_window_with_margin(px(space::S2))
                .child(body),
            None => anchored()
                .anchor(match (menu.kind.opens_up(), menu.kind.aligns_left()) {
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

    /// The list picker's filter line: what's been typed, or a hint that typing narrows the lists.
    fn filter_line(&self, p: &Palette, typed: SharedString) -> Div {
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
                        SharedString::from("Type to filter lists")
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
        let opens_up = self.menu.as_ref().is_some_and(|m| m.kind.opens_up());
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
        let ink = if item.danger { p.danger_ink } else { p.ink };
        let action = item.action;
        let hover = p.hover;
        div()
            .id(item.key.clone())
            .mx(px(space::S1))
            .h(px(desktop::NAV_ROW))
            .px(px(space::S2))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .border_color(if highlighted {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .when(highlighted, |d| d.bg(p.hover))
            .style(ty::NAV_SUB)
            .text_color(ink)
            .child(match item.glyph {
                // A list's own glyph in its colour, as in the sidebar.
                Some((glyph, hue)) => kit::glyph(glyph, theme::list_colors(self.theme, hue).glyph),
                None => kit::icon(
                    item.icon,
                    component::ICON_SMALL,
                    if item.danger { p.danger } else { p.ink_3 },
                ),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(item.label.clone()),
            )
            .children(item.hint.clone().map(|hint| {
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

/// What a key does to the list picker's filter (Move to…): the filter it leaves, or `None` when the key
/// isn't typing there. Letters add to it, Backspace takes the last one back, and Escape clears it before
/// it closes the picker. Space types once something is typed (before that it chooses, as in any menu).
fn picker_edit(menu: &Menu, e: &KeyDownEvent) -> Option<String> {
    let picking = matches!(
        menu.kind,
        MenuKind::Task {
            page: TaskPage::Move,
            ..
        }
    );
    let m = &e.keystroke.modifiers;
    if !picking || m.control || m.alt || m.platform || m.function {
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
        let room = MenuKind::Room { confirm: false };
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
        guard.closed(MenuKind::Room { confirm: true });
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
        let room = MenuKind::Room { confirm: false };
        let mut guard = PressGuard::default();
        guard.press();
        guard.closed(MenuKind::Account { confirm: false });
        assert!(!guard.swallows(room));
        guard.press();
        guard.closed(MenuKind::List {
            id: list,
            confirm: false,
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
        };
        let key = |text: &'static str| KeyDownEvent {
            keystroke: Keystroke::parse(text).expect("keystroke"),
            is_held: false,
            prefer_character_input: false,
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
        assert!(MenuKind::Room { confirm: false }.same_menu(MenuKind::Room { confirm: true }));
        assert!(
            MenuKind::Account { confirm: false }.same_menu(MenuKind::Account { confirm: true })
        );
        assert!(
            MenuKind::List {
                id: list,
                confirm: false
            }
            .same_menu(MenuKind::List {
                id: list,
                confirm: true
            })
        );
        assert!(!MenuKind::Room { confirm: false }.same_menu(MenuKind::Account { confirm: false }));
        // Only the account menu opens upwards (from the sidebar footer).
        assert!(MenuKind::Account { confirm: false }.opens_up());
        assert!(!MenuKind::Room { confirm: false }.opens_up());
    }
}
