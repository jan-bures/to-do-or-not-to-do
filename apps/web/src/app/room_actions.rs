//! List and collection actions, and the sidebar's list of refused changes.

use super::{Ctx, RoomId, backend, ctx, glyph, icon, model, pickers, platform, sync_text};
use leptos::html;
use leptos::prelude::*;
use todo_client::{CommandId, intent};
use todo_core::ids::{CollectionId, EntityId, ListId};
use todo_core::query::fold;
use todo_core::{Command, State};
use todo_design::{Glyph, Icon};
use wasm_bindgen::JsCast;

const MENU_ID: &str = "room-action-menu";
const PICKER_ID: &str = "room-action-picker";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Subject {
    List(ListId),
    Collection(CollectionId),
}

impl Subject {
    pub(super) fn of(room: &RoomId) -> Option<Self> {
        match *room {
            RoomId::List(id) => Some(Self::List(id)),
            RoomId::Collection(id) => Some(Self::Collection(id)),
            RoomId::Smart(_) | RoomId::Activity(_) => None,
        }
    }

    fn id(self) -> EntityId {
        match self {
            Self::List(id) => id.into(),
            Self::Collection(id) => id.into(),
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::List(_) => "list",
            Self::Collection(_) => "collection",
        }
    }

    fn name(self, state: &State) -> Option<&str> {
        if !state.is_live(self.id()) {
            return None;
        }
        match self {
            Self::List(id) => state.lists.get(&id).map(|l| l.name.as_str()),
            Self::Collection(id) => state.collections.get(&id).map(|c| c.name.as_str()),
        }
    }

    fn hue(self, state: &State) -> Option<&'static str> {
        let (colour, id) = match self {
            Self::List(id) => (state.lists.get(&id)?.color.as_deref(), id.as_uuid()),
            Self::Collection(id) => (state.collections.get(&id)?.color.as_deref(), id.as_uuid()),
        };
        Some(model::swatch(colour, id.as_bytes()))
    }

    fn rename(self, state: &State, name: &str) -> Option<Command> {
        self.name(state)?;
        match self {
            Self::List(id) => intent::rename_list(state, id, name),
            Self::Collection(id) => intent::rename_collection(state, id, name),
        }
    }

    fn fallback(self, state: &State) -> RoomId {
        match self {
            Self::List(id) => state
                .lists
                .get(&id)
                .and_then(|l| l.collection)
                .filter(|id| state.is_live((*id).into()))
                .map_or_else(RoomId::default, RoomId::Collection),
            Self::Collection(_) => RoomId::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Anchor {
    Header,
    Link,
    Sidebar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Origin(pub(super) Subject, pub(super) Anchor);

impl Origin {
    pub(super) fn dom_id(self) -> String {
        match self.1 {
            Anchor::Header => "open-room-menu".into(),
            Anchor::Link => format!("sidebar-room-{}", self.0.id()),
            Anchor::Sidebar => format!("sidebar-menu-{}", self.0.id()),
        }
    }

    fn restore_focus(self, controls: Controls) {
        let previous = pickers::focused();
        request_animation_frame(move || {
            // Wait for a reordered sidebar to render, but don't steal focus from a control
            // reached in the meantime. A removed opener can leave focus on the body.
            if pickers::focus_unmoved(previous.as_ref())
                && !controls.dialog_open()
                && controls.menu.with_untracked(Option::is_none)
                && !platform::focus(&self.dom_id())
                && !platform::focus("open-drawer")
            {
                platform::focus("room-title");
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Rename,
    Up,
    Down,
    Colour,
    MoveTo,
    Fold,
    Completed,
    Delete,
}

impl Item {
    fn id(self) -> &'static str {
        match self {
            Self::Rename => "menu-rename",
            Self::Up => "menu-move-up",
            Self::Down => "menu-move-down",
            Self::Colour => "menu-colour",
            Self::MoveTo => "menu-move-to",
            Self::Fold => "menu-fold",
            Self::Completed => "menu-completed",
            Self::Delete => "menu-delete",
        }
    }
}

#[derive(Clone, Copy)]
struct Menu {
    origin: Origin,
    highlighted: Item,
}

impl Menu {
    fn items(self) -> Vec<Item> {
        let mut items = vec![Item::Rename, Item::Up, Item::Down, Item::Colour];
        if matches!(self.origin.0, Subject::List(_)) {
            items.push(Item::MoveTo);
        }
        if self.origin.1 == Anchor::Header {
            items.push(Item::Completed);
        } else if matches!(self.origin.0, Subject::Collection(_)) {
            items.push(Item::Fold);
        }
        items.push(Item::Delete);
        items
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Actions,
    Colour,
    Move,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Rename(Origin),
    Delete(Origin),
}

impl Edit {
    fn origin(self) -> Origin {
        match self {
            Self::Rename(origin) | Self::Delete(origin) => origin,
        }
    }

    fn title(self) -> String {
        let action = match self {
            Self::Rename(_) => "Rename",
            Self::Delete(_) => "Delete",
        };
        format!("{action} {}", self.origin().0.kind())
    }
}

#[derive(Clone, Copy)]
pub(super) struct Controls {
    menu: RwSignal<Option<Menu>>,
    page: RwSignal<Page>,
    popover: NodeRef<html::Div>,
    picker: NodeRef<html::Div>,
    edit: RwSignal<Option<Edit>>,
    edit_dialog: NodeRef<html::Dialog>,
    failed_dialog: NodeRef<html::Dialog>,
    /// Stable snapshot: the IDs on offer during this opening of the dialog.
    seen: RwSignal<Option<Vec<CommandId>>>,
}

impl Controls {
    pub(super) fn new() -> Self {
        Self {
            menu: RwSignal::new(None),
            page: RwSignal::new(Page::Actions),
            popover: NodeRef::new(),
            picker: NodeRef::new(),
            edit: RwSignal::new(None),
            edit_dialog: NodeRef::new(),
            failed_dialog: NodeRef::new(),
            seen: RwSignal::new(None),
        }
    }

    pub(super) fn dialog_open(self) -> bool {
        self.edit.with_untracked(Option::is_some) || self.seen.with_untracked(Option::is_some)
    }

    pub(super) fn anchored(self, origin: Origin) -> bool {
        self.menu
            .with(|menu| menu.is_some_and(|m| m.origin == origin))
    }

    pub(super) fn open_menu(self, c: Ctx, origin: Origin) {
        let Some(el) = self.popover.get_untracked() else {
            return;
        };
        if c.read_only() || c.state.with_untracked(|s| origin.0.name(s).is_none()) {
            return;
        }
        if self
            .menu
            .with_untracked(|m| m.is_some_and(|m| m.origin == origin))
            && self
                .active_popover()
                .is_some_and(|el| el.matches(":popover-open").unwrap_or(false))
        {
            self.close_menu(true);
            return;
        }
        if let Some(picker) = self.picker.get_untracked() {
            let _ = picker.hide_popover();
        }
        self.page.set(Page::Actions);
        self.menu.set(Some(Menu {
            origin,
            highlighted: Item::Rename,
        }));
        if el.show_popover().is_err() {
            self.menu.set(None);
            return;
        }
        self.position_menu();
        request_animation_frame(move || {
            if self.menu.with_untracked(|m| {
                m.is_some_and(|m| m.origin == origin && m.highlighted == Item::Rename)
            }) {
                platform::focus(Item::Rename.id());
            }
        });
    }

    fn active_popover(self) -> Option<leptos::web_sys::HtmlDivElement> {
        if self.page.get_untracked() == Page::Actions {
            self.popover.get_untracked()
        } else {
            self.picker.get_untracked()
        }
    }

    fn toggle(self, is_menu: bool) {
        // Moving between surfaces queues a close event on the old one; ignore it.
        if (self.page.get_untracked() == Page::Actions) == is_menu
            && self
                .active_popover()
                .is_some_and(|el| !el.matches(":popover-open").unwrap_or(false))
        {
            self.menu.set(None);
        }
    }

    fn page(self, page: Page, focus: Option<Item>) {
        if let Some(el) = self.active_popover() {
            let _ = el.hide_popover();
        }
        self.page.set(page);
        if self
            .active_popover()
            .is_none_or(|el| el.show_popover().is_err())
        {
            self.close_menu(true);
            return;
        }
        request_animation_frame(move || self.position_menu());
        if let Some(item) = focus {
            request_animation_frame(move || {
                if self.menu.with_untracked(Option::is_some) && self.page.get_untracked() == page {
                    platform::focus(item.id());
                }
            });
        } else {
            pickers::focus_choice(PICKER_ID, "room-picker-back");
        }
    }

    pub(super) fn position_menu(self) {
        if let Some((menu, el)) = self.menu.get_untracked().zip(self.active_popover())
            && !platform::place_room_menu(&el, &menu.origin.dom_id())
        {
            self.close_menu(false);
        }
    }

    pub(super) fn close_menu(self, restore: bool) {
        if let Some(menu) = self.menu.try_update(Option::take).flatten() {
            if let Some(el) = self.active_popover() {
                let _ = el.hide_popover();
            }
            if restore {
                menu.origin.restore_focus(self);
            }
        }
    }

    fn open_edit(self, edit: Edit) {
        let Some(dialog) = self.edit_dialog.get_untracked() else {
            return;
        };
        self.edit.set(Some(edit));
        if dialog.show_modal().is_err() {
            self.edit.set(None);
            return;
        }
        request_animation_frame(move || match edit {
            Edit::Rename(_) => platform::focus_select_text("rename-room-name"),
            Edit::Delete(_) => {
                platform::focus("cancel-room-action");
            }
        });
    }

    fn open_failures(self, c: Ctx) {
        let Some(dialog) = self.failed_dialog.get_untracked() else {
            return;
        };
        let ids: Vec<_> = c
            .failures
            .with_untracked(|failures| failures.iter().map(|f| f.id).collect());
        if ids.is_empty() {
            return;
        }
        self.seen.set(Some(ids));
        if dialog.show_modal().is_err() {
            self.seen.set(None);
        }
    }
}

pub(super) fn context_key(ev: &leptos::ev::KeyboardEvent) -> bool {
    ev.key() == "ContextMenu" || (ev.key() == "F10" && ev.shift_key())
}

#[component]
pub(super) fn MenuButton(origin: Origin, label: String) -> impl IntoView {
    let c = ctx();
    let controls = c.room_actions;
    view! {
        <button type="button" id=origin.dom_id() class="icon-button room-menu-trigger"
            class:menu-anchor=move || controls.anchored(origin)
            popovertarget=MENU_ID aria-haspopup="menu" aria-controls=MENU_ID
            aria-expanded=move || (controls.page.get() == Page::Actions && controls.anchored(origin)).to_string()
            aria-label=label.clone() title=label disabled=move || c.read_only()
            on:click=move |ev| { ev.prevent_default(); controls.open_menu(c, origin); }
            on:keydown=move |ev| {
                if context_key(&ev) {
                    ev.prevent_default();
                    if !ev.repeat() { controls.open_menu(c, origin); }
                }
            }
        >{icon(Icon::Ellipsis)}</button>
    }
}

#[component]
pub(super) fn RoomMenuButton() -> impl IntoView {
    let c = ctx();
    let subject = Memo::new(move |_| {
        c.room
            .with(Subject::of)
            .filter(|s| c.state.with(|state| s.name(state).is_some()))
    });
    move || {
        subject.get().map(|subject| {
            view! {
                <MenuButton origin=Origin(subject, Anchor::Header) label="Room menu".to_owned()/>
            }
        })
    }
}

#[component]
pub(super) fn SyncLine() -> impl IntoView {
    let c = ctx();
    let label = Memo::new(move |_| sync_text(&c.sync.get(), c.synced_at.get()));
    let full = move || label.with(|l| l.full.clone());
    let short = move || label.with(|l| l.short.clone());
    view! {
        <Show when=move || c.failures.with(|f| !f.is_empty()) fallback=move || view! {
            <span class="sync-text" role="status" title=full aria-label=full>
                <span aria-hidden="true">{short}</span>
            </span>
        }>
            <button type="button" class="sync-text sync-failures-trigger" id="open-sync-failures"
                aria-haspopup="dialog" aria-controls="sync-failures" title=full aria-label=full
                on:click=move |_| c.room_actions.open_failures(c)>{short}</button>
        </Show>
    }
}

#[component]
pub(super) fn Overlays() -> impl IntoView {
    view! { <ActionMenu/><EditDialog/><FailedDialog/> }
}

#[component]
fn ActionMenu() -> impl IntoView {
    let c = ctx();
    let controls = c.room_actions;
    // Items don't rebuild on sync, and each action keeps both its item ID and its subject's ID.
    let origin = Memo::new(move |_| controls.menu.with(|m| m.map(|m| m.origin)));
    let resize = window_event_listener(leptos::ev::resize, move |_| controls.position_menu());
    on_cleanup(move || resize.remove());
    Effect::new(move |_| {
        if origin.get().is_some() {
            c.sidebar.track();
            c.view_room.track();
            c.collapsed.track();
            request_animation_frame(move || controls.position_menu());
        }
    });
    Effect::new(move |_| {
        if let Some(origin) = origin.get()
            && (c.read_only() || c.state.with(|s| origin.0.name(s).is_none()))
        {
            controls.close_menu(true);
        }
    });
    view! {
        <div id=MENU_ID role="menu" aria-label="Room actions" class="room-action-menu"
            inert=move || controls.page.get() != Page::Actions || origin.get().is_none()
            popover="auto" node_ref=controls.popover on:toggle=move |_| controls.toggle(true)
            on:keydown=move |ev| menu_key(c, &ev)>
            {move || origin.get().map(|origin| Menu { origin, highlighted: Item::Rename }.items().into_iter().map(|item| view! {
                <MenuItem origin=origin item=item/>
            }).collect_view())}
        </div>
        <div id=PICKER_ID role="dialog" aria-labelledby="room-picker-heading"
            inert=move || controls.page.get() == Page::Actions || origin.get().is_none()
            class="room-action-menu" class:swatch-picker=move || controls.page.get() == Page::Colour
            popover="auto" node_ref=controls.picker on:toggle=move |_| controls.toggle(false)
            on:keydown=move |ev| menu_key(c, &ev)>
            {move || origin.get().map(|origin| match controls.page.get() {
                Page::Actions => ().into_any(),
                Page::Colour => view! { <ColourPicker origin=origin/> }.into_any(),
                Page::Move => match origin.0 {
                    Subject::List(id) => view! { <MovePicker origin=origin id=id/> }.into_any(),
                    Subject::Collection(_) => ().into_any(),
                },
            })}
        </div>
    }
}

fn menu_key(c: Ctx, ev: &leptos::ev::KeyboardEvent) {
    ev.stop_propagation();
    if ev.is_composing() {
        return;
    }
    let controls = c.room_actions;
    let Some(menu) = controls.menu.get_untracked() else {
        return;
    };
    let key = ev.key();
    if ev.repeat() {
        ev.prevent_default();
        return;
    }
    if ev.ctrl_key() && !ev.alt_key() && !ev.shift_key() && key == "z" && !c.read_only() {
        ev.prevent_default();
        controls.close_menu(true);
        c.undo();
        return;
    }
    let page = controls.page.get_untracked();
    if page != Page::Actions {
        match key.as_str() {
            "Escape" => {
                ev.prevent_default();
                controls.close_menu(true);
            }
            "Tab" => platform::contain_tab(PICKER_ID, ev),
            _ => pickers::choice_key(PICKER_ID, if page == Page::Colour { 6 } else { 1 }, ev),
        }
        return;
    }
    let items = menu.items();
    let current = items
        .iter()
        .position(|i| *i == menu.highlighted)
        .unwrap_or_default();
    let next = match key.as_str() {
        "Escape" => {
            ev.prevent_default();
            controls.close_menu(true);
            return;
        }
        "Tab" => {
            controls.close_menu(false);
            platform::focus(&menu.origin.dom_id());
            return;
        }
        "ArrowDown" => (current + 1) % items.len(),
        "ArrowUp" => (current + items.len() - 1) % items.len(),
        "Home" => 0,
        "End" => items.len() - 1,
        _ => return,
    };
    ev.prevent_default();
    platform::focus(items[next].id());
}

#[component]
fn MenuItem(origin: Origin, item: Item) -> impl IntoView {
    let c = ctx();
    let controls = c.room_actions;
    let subject = origin.0;
    let label = move || match item {
        Item::Rename => format!("Rename {}", subject.kind()),
        Item::Up => "Move up".into(),
        Item::Down => "Move down".into(),
        Item::Colour => "Colour…".into(),
        Item::MoveTo => "Move to…".into(),
        Item::Fold => {
            if matches!(subject, Subject::Collection(id) if c.collapsed.with(|ids| ids.contains(&id)))
            {
                "Expand collection".into()
            } else {
                "Collapse collection".into()
            }
        }
        Item::Completed if c.completed_open.get() => "Hide completed".into(),
        Item::Completed => "Show completed".into(),
        Item::Delete => format!("Delete {}", subject.kind()),
    };
    let disabled = move || match item {
        Item::Up | Item::Down => c.state.with(|s| {
            intent::nudge(s, subject.id(), if item == Item::Up { -1 } else { 1 }).is_none()
        }),
        Item::Completed => c.view_room.with(|r| r.completed.is_empty()),
        Item::MoveTo => {
            matches!(subject, Subject::List(id) if !c.state.with(|s| intent::organize::can_move_list(s, id)))
        }
        _ => false,
    };
    let glyph = match item {
        Item::Rename => Icon::Pencil,
        Item::Up => Icon::ChevronUp,
        Item::Down => Icon::ChevronDown,
        Item::Colour => Icon::Pencil,
        Item::MoveTo => Icon::ChevronRight,
        Item::Fold => Icon::ChevronRight,
        Item::Completed => Icon::Check,
        Item::Delete => Icon::TrashTwo,
    };
    view! {
        <button id=item.id() type="button" role="menuitem" tabindex="-1"
            aria-haspopup=matches!(item, Item::Colour | Item::MoveTo).then_some("dialog")
            class:danger=item == Item::Delete aria-disabled=move || disabled().to_string()
            on:focus=move |_| controls.menu.update(|m| {
                if let Some(m) = m.as_mut().filter(|m| m.origin == origin) { m.highlighted = item; }
            })
            on:click=move |_| {
                if !disabled() && controls.menu.with_untracked(|m| m.is_some_and(|m| m.origin == origin)) {
                    activate(c, origin, item);
                }
            }>
            {if item == Item::Colour {
                view! { <span class="tag-dot" data-list-hue=move || c.state.with(|s| subject.hue(s))></span> }.into_any()
            } else { icon(glyph).into_any() }}
            {label}
        </button>
    }
}

fn activate(c: Ctx, origin: Origin, item: Item) {
    let controls = c.room_actions;
    let subject = origin.0;
    match item {
        Item::Colour => return controls.page(Page::Colour, None),
        Item::MoveTo => return controls.page(Page::Move, None),
        _ => controls.close_menu(false),
    }
    match item {
        Item::Colour | Item::MoveTo => {}
        Item::Rename => controls.open_edit(Edit::Rename(origin)),
        Item::Delete => controls.open_edit(Edit::Delete(origin)),
        Item::Up | Item::Down => {
            let by = if item == Item::Up { -1 } else { 1 };
            if let Some((cmd, label)) = c.state.with_untracked(|s| {
                Some((
                    intent::nudge(s, subject.id(), by)?,
                    format!("Moved “{}”", subject.name(s)?),
                ))
            }) {
                c.submit_undoable(cmd, label, None);
            }
            // Keep the drawer open and return to the very list/collection that moved.
            origin.restore_focus(controls);
        }
        Item::Fold => {
            if let Subject::Collection(id) = subject {
                c.toggle_fold(id);
            }
            origin.restore_focus(controls);
        }
        Item::Completed => {
            c.completed_open.update(|open| *open = !*open);
            origin.restore_focus(controls);
        }
    }
}

#[component]
fn ColourPicker(origin: Origin) -> impl IntoView {
    let c = ctx();
    view! {
        <div class="picker-page">
            <h3 id="room-picker-heading">{intent::organize::colour_heading(origin.0.id())}</h3>
            <pickers::Swatches selected=Signal::derive(move || c.state.with(|s| origin.0.hue(s)))
                disabled=Signal::derive(move || c.read_only())
                choose=Callback::new(move |key| {
                    if c.read_only() { return; }
                    let cmd = c.state.with_untracked(|s| intent::organize::set_color(s, origin.0.id(), key));
                    if cmd.is_none_or(|cmd| c.submit_undoable(cmd, "Colour changed.".into(), None)) {
                        c.room_actions.close_menu(true);
                    }
                })/>
            <PickerBack item=Item::Colour/>
        </div>
    }
}

#[component]
fn MovePicker(origin: Origin, id: ListId) -> impl IntoView {
    let c = ctx();
    let ids = Memo::new(move |_| c.state.with(|s| intent::organize::collections(s)));
    let choose = Callback::new(move |to: Option<CollectionId>| {
        if c.read_only() {
            return;
        }
        let change = c.state.with_untracked(|s| {
            Some((
                intent::organize::move_list(s, id, to)?,
                format!("Moved “{}”", origin.0.name(s)?),
            ))
        });
        if let Some((cmd, label)) = change {
            if !c.submit_undoable(cmd, label, None) {
                return;
            }
            if let Some(to) = to {
                c.fold(to, false);
            }
        }
        c.room_actions.close_menu(true);
    });
    view! {
        <div class="picker-page">
            <h3 id="room-picker-heading">"Move to"</h3>
            <div role="listbox" aria-labelledby="room-picker-heading" on:focusin=pickers::choice_focus>
                <For each=move || ids.get() key=|id| *id children=move |to| view! { <MoveChoice list=id to=Some(to) choose=choose/> }/>
                <MoveChoice list=id to=None choose=choose/>
            </div>
            <PickerBack item=Item::MoveTo/>
        </div>
    }
}

#[component]
fn MoveChoice(
    list: ListId,
    to: Option<CollectionId>,
    choose: Callback<Option<CollectionId>>,
) -> impl IntoView {
    let c = ctx();
    let name = move || {
        c.state.with(|s| {
            to.and_then(|id| s.collections.get(&id).map(|c| c.name.clone()))
                .unwrap_or_else(|| "No collection".into())
        })
    };
    let hue = move || {
        c.state
            .with(|s| to.and_then(|id| Subject::Collection(id).hue(s)))
    };
    view! {
        <button type="button" data-picker-option="" class="move-choice" role="option" tabindex="-1"
            disabled=move || c.read_only() || c.state.with(|s| to.is_some_and(|id| !s.is_live(id.into())))
            aria-selected=move || c.state.with(|s| s.lists.get(&list).is_some_and(|l| l.collection == to)).to_string()
            on:click=move |_| choose.run(to)>
            <span data-list-hue=hue class="move-glyph">{move || {
                let icon = to.map_or(Glyph::Layers, |_| Glyph::for_collection(&fold(&name())));
                glyph(icon)
            }}</span>
            <span class="picker-label">{name}</span>
            <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
        </button>
    }
}

#[component]
fn PickerBack(item: Item) -> impl IntoView {
    let controls = ctx().room_actions;
    view! {
        <button type="button" id="room-picker-back" on:click=move |_| controls.page(Page::Actions, Some(item))>
            {icon(Icon::ChevronLeft)}"Back"
        </button>
    }
}

pub(super) fn scrim_click(ev: &leptos::ev::MouseEvent) -> bool {
    let Some(dialog) = ev
        .target()
        .and_then(|el| el.dyn_into::<web_sys::HtmlDialogElement>().ok())
    else {
        return false;
    };
    let r = dialog.get_bounding_client_rect();
    let (x, y) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
    x < r.left() || x >= r.right() || y < r.top() || y >= r.bottom()
}

pub(super) fn close_dialog(node: NodeRef<html::Dialog>) {
    if let Some(dialog) = node.get_untracked() {
        dialog.close();
    }
}

pub(super) fn block_dialog_repeat(ev: &leptos::ev::KeyboardEvent) {
    if ev.repeat() && matches!(ev.key().as_str(), "Enter" | "Escape") {
        ev.prevent_default();
    }
}

#[component]
fn EditDialog() -> impl IntoView {
    let c = ctx();
    let controls = c.room_actions;
    let close = move || close_dialog(controls.edit_dialog);
    let input = NodeRef::<html::Input>::new();
    let error = RwSignal::new(None::<String>);
    let saving_focus = RwSignal::new(false);
    view! {
        <dialog class="room-dialog" node_ref=controls.edit_dialog aria-labelledby="room-action-heading"
            aria-describedby="room-action-description"
            on:keydown=move |ev| block_dialog_repeat(&ev)
            on:click=move |ev| { if scrim_click(&ev) { close(); } }
            on:close=move |_| {
                error.set(None);
                if let Some(edit) = controls.edit.try_update(Option::take).flatten()
                    && !saving_focus.try_update(|v| std::mem::replace(v, false)).unwrap_or(false)
                {
                    edit.origin().restore_focus(controls);
                }
            }>
            {move || controls.edit.get().map(|edit| {
                let origin = edit.origin();
                let subject = origin.0;
                let rename = matches!(edit, Edit::Rename(_));
                let original = c.state.with_untracked(|s| subject.name(s).unwrap_or_default().to_owned());
                let shown_name = original.clone();
                let available = move || c.state.with(|s| subject.name(s).is_some());
                view! {
                    <form on:submit=move |ev| {
                        ev.prevent_default();
                        if c.read_only() || !available() { return; }
                        if rename {
                            let Some(input) = input.get_untracked() else { return };
                            let name = input.value();
                            if name.trim().is_empty() { error.set(Some("Enter a name.".into())); return; }
                            if name.trim() == original { close(); return; }
                            let cmd = c.state.with_untracked(|s| subject.rename(s, &name));
                            if cmd.is_none_or(|cmd| c.submit(cmd)) {
                                close();
                            } else {
                                error.set(c.toast.get_untracked().map(|toast| toast.text));
                            }
                        } else {
                            let name = c.state.with_untracked(|s| subject.name(s).unwrap_or_default().to_owned());
                            if !c.submit_undoable(intent::delete(subject.id()), format!("Deleted “{name}”"), None) {
                                error.set(c.toast.get_untracked().map(|toast| toast.text));
                                return;
                            }
                            let active = c.room.with_untracked(Subject::of);
                            if let Some(active) = active.filter(|active| c.state.with_untracked(|s| !s.is_live(active.id()))) {
                                let fallback = c.state.with_untracked(|s| active.fallback(s));
                                c.go(fallback);
                            }
                            c.drawer.set(false);
                            saving_focus.set(true);
                            close();
                            request_animation_frame(|| { platform::focus("room-title"); });
                        }
                    }>
                        <h2 id="room-action-heading">{edit.title()}</h2>
                        <div id="room-action-description">
                            <Show when=move || !available()>
                                <p>"This item is no longer available."</p>
                            </Show>
                            {(!rename).then(|| view! {
                                <Show when=available>
                                    <p>{move || c.state.with(|s| subject.name(s).map(|name| format!("Delete “{name}”?")))}</p>
                                    <p>{match subject {
                                        Subject::List(_) => "Its tasks will be hidden too. You can undo this change.",
                                        Subject::Collection(_) => "Its lists and tasks will be hidden too. You can undo this change.",
                                    }}</p>
                                </Show>
                            })}
                        </div>
                        {rename.then(|| view! {
                            <input type="text" value=shown_name node_ref=input id="rename-room-name"
                                class="room-name-input" aria-label=format!("{} name", subject.kind())
                                autocomplete="off" required disabled=move || c.read_only() || !available()
                                on:input=move |_| error.set(None)/>
                        })}
                        {move || error.get().map(|message| view! {
                            <p role="alert" class="action-error">{message}</p>
                        })}
                        <div class="dialog-actions">
                            <button type="button" class="button" id="cancel-room-action" on:click=move |_| close()>"Cancel"</button>
                            <button type="submit" class="button" class:danger=!rename class:primary=rename
                                disabled=move || c.read_only() || !available()>{if rename { "Save" } else { "Delete" }}</button>
                        </div>
                    </form>
                }
            })}
        </dialog>
    }
}

#[component]
fn FailedDialog() -> impl IntoView {
    let c = ctx();
    let controls = c.room_actions;
    let error = RwSignal::new(false);
    let shown = Memo::new(move |_| {
        controls.seen.with(|seen| {
            c.failures.with(|all| {
                all.iter()
                    .filter(|f| seen.as_ref().is_some_and(|ids| ids.contains(&f.id)))
                    .cloned()
                    .collect::<Vec<_>>()
            })
        })
    });
    let close = move || close_dialog(controls.failed_dialog);
    let dismiss = move |ids: &[CommandId]| {
        error.set(backend::dismiss_failures(ids).is_err());
        c.refresh();
        request_animation_frame(|| {
            if !platform::focus("dismiss-all-failures") {
                platform::focus("close-sync-failures");
            }
        });
    };
    view! {
        <dialog id="sync-failures" class="room-dialog failure-dialog" node_ref=controls.failed_dialog
            aria-labelledby="sync-failures-heading"
            on:keydown=move |ev| block_dialog_repeat(&ev)
            on:click=move |ev| { if scrim_click(&ev) { close(); } }
            on:close=move |_| {
                controls.seen.set(None);
                error.set(false);
                if !platform::focus("open-sync-failures") { platform::focus("open-new-room"); }
            }>
            <h2 id="sync-failures-heading">"Not synced"</h2>
            <Show when=move || shown.with(|rows| rows.is_empty())>
                <p role="status">"No more changes to dismiss."</p>
            </Show>
            <ul class="failure-list">
                <For each=move || shown.get() key=|f| f.id let:f>
                    <li>
                        <div><strong>{f.what}</strong><p>{f.why}</p></div>
                        <button type="button" class="button" disabled=move || c.read_only()
                            on:click=move |_| dismiss(&[f.id])>"Dismiss"</button>
                    </li>
                </For>
            </ul>
            <Show when=move || error.get()>
                <p role="alert" class="action-error">"Couldn’t dismiss those changes."</p>
            </Show>
            <div class="dialog-actions">
                <button type="button" class="button" id="close-sync-failures" autofocus on:click=move |_| close()>"Close"</button>
                {move || {
                    // This closure holds the IDs from this render even if sync adds another refusal.
                    let ids = shown.with(|rows| rows.iter().map(|f| f.id).collect::<Vec<_>>());
                    (!ids.is_empty()).then(|| view! {
                        <button type="button" id="dismiss-all-failures" class="button" disabled=move || c.read_only()
                            on:click=move |_| dismiss(&ids)>"Dismiss all"</button>
                    })
                }}
            </div>
        </dialog>
    }
}
