//! The Leptos components. Layout and look are in `static/app.css`, which takes every value from
//! `tokens.css`; this file only sets classes and `data-*` hue keys.

use crate::backend::{self, Sync};
use crate::landscape::{self, Sky};
use crate::model::{self, AgentMark, Detail, DueTone, Now, Room, RoomId, Row, Sidebar, Source};
use crate::platform;
use leptos::html;
use leptos::prelude::*;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use todo_client::Connection;
use todo_core::command::{NewTask, Placement, TaskPatch};
use todo_core::ids::{CollectionId, ListId, StepId, TaskId};
use todo_core::model::Priority;
use todo_core::{Command, State};
use todo_design::landscape::{HEIGHT_COVER, HEIGHT_DESKTOP, Landscape};
use todo_design::{Glyph, Icon, motion};

/// Where a completing row is in its choreography.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Fill, tick and hold: the row stays in place, checked and struck through.
    Hold,
    /// Collapsing out of the list.
    Exit,
}

/// A row that was just completed and is still shown where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linger {
    room: RoomId,
    row: Row,
    index: usize,
    phase: Phase,
}

/// A row as the list renders it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shown {
    row: Row,
    phase: Option<Phase>,
}

/// The toast at the bottom of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    id: u64,
    text: String,
    /// The task to reopen on Undo.
    undo: Option<TaskId>,
}

/// Shared UI state. Everything is a signal, so the struct is `Copy`.
#[derive(Clone, Copy)]
pub struct Ctx {
    state: RwSignal<Arc<State>>,
    sync: RwSignal<Sync>,
    synced_at: RwSignal<Option<Now>>,
    now: RwSignal<Now>,
    room: RwSignal<RoomId>,
    selected: RwSignal<Option<TaskId>>,
    lingering: RwSignal<Vec<Linger>>,
    toast: RwSignal<Option<Toast>>,
    toast_seq: RwSignal<u64>,
    dark: RwSignal<bool>,
    drawer: RwSignal<bool>,
    narrow: RwSignal<bool>,
    detail_modal: Memo<bool>,
    collapsed: RwSignal<BTreeSet<CollectionId>>,
    new_list: RwSignal<bool>,
    filter_open: RwSignal<bool>,
    filter_text: RwSignal<String>,
    completed_open: RwSignal<bool>,
    columns: Memo<Columns>,
    editing: RwSignal<bool>,
    quick_add: NodeRef<html::Input>,
    view_room: Memo<Room>,
    finished: Memo<bool>,
    shown: Memo<Vec<Shown>>,
    detail: Memo<Option<Detail>>,
    sidebar: Memo<Sidebar>,
}

fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

impl Ctx {
    /// Reads the backend's view and status into the signals.
    fn refresh(self) {
        self.state.set(backend::view());
        let s = backend::status();
        if s.connection == Connection::Online && !s.offline_demo {
            self.synced_at.set(Some(platform::now()));
        }
        if s.offline_demo && self.synced_at.get_untracked().is_none() {
            self.synced_at.set(Some(self.now.get_untracked()));
        }
        if s.signed_out {
            platform::sign_in(&platform::path());
        }
        self.sync.set(s);
    }

    /// A newer version took the database over: no more edits here (see `backend::submit`).
    fn read_only(self) -> bool {
        self.sync.with(|s| s.read_only)
    }

    fn submit(self, cmd: Command) -> bool {
        match backend::submit(cmd) {
            Ok(()) => {
                self.refresh();
                true
            }
            // Nothing was applied; the persistent alert says why and offers a reload.
            Err(_) if backend::status().read_only => {
                self.refresh();
                false
            }
            Err(e) => {
                tracing::warn!("command refused: {e}");
                self.show_toast(format!("Couldn’t save that: {e}"), None);
                false
            }
        }
    }

    fn show_toast(self, text: String, undo: Option<TaskId>) {
        let id = self.toast_seq.get_untracked() + 1;
        self.toast_seq.set(id);
        self.toast.set(Some(Toast { id, text, undo }));
        set_timeout(
            move || {
                if self.toast.get_untracked().is_some_and(|t| t.id == id) {
                    self.toast.set(None);
                }
            },
            Duration::from_millis(u64::from(motion::UNDO_WINDOW)),
        );
    }

    fn reset_room_controls(self) {
        self.drawer.set(false);
        self.new_list.set(false);
        self.filter_open.set(false);
        self.filter_text.set(String::new());
        self.completed_open.set(false);
    }

    fn go(self, room: RoomId) {
        self.reset_room_controls();
        self.selected.set(None);
        self.editing.set(false);
        platform::push_path(&room.path());
        self.room.set(room);
    }

    fn select(self, id: Option<TaskId>) {
        self.editing.set(false);
        self.selected.set(id);
        let room = self.room.get_untracked();
        let path = match id {
            Some(t) => format!("{}/task/{t}", room.path().trim_end_matches('/')),
            None => room.path(),
        };
        platform::push_path(&path);
    }

    fn open_filter(self) {
        if self.filter_open.get_untracked() {
            platform::focus("room-filter");
        } else {
            self.filter_open.set(true);
            request_animation_frame(|| {
                platform::focus("room-filter");
            });
        }
    }

    fn close_filter(self) {
        self.filter_text.set(String::new());
        self.filter_open.set(false);
        request_animation_frame(|| {
            platform::focus("open-filter");
        });
    }

    /// Completes a task with the F2 choreography: the row holds in place, then collapses; the sun moves
    /// when it has gone.
    fn complete(self, row: &Row) {
        let room = self.room.get_untracked();
        let undoable = self
            .state
            .with_untracked(|s| model::reopen_undoes_completion(s, row.id));
        let index = self
            .shown
            .get_untracked()
            .iter()
            .position(|s| s.row.id == row.id)
            .unwrap_or(0);
        if !self.submit(Command::CompleteTask { id: row.id }) {
            return;
        }
        let id = row.id;
        let mut done = row.clone();
        done.done = true;
        self.lingering.update(|l| {
            l.retain(|x| x.row.id != id);
            l.push(Linger {
                room,
                row: done,
                index,
                phase: Phase::Hold,
            });
        });
        let hold = motion::CHECK_FILL
            + motion::CHECK_TICK_DELAY
            + motion::CHECK_TICK
            + motion::COMPLETE_HOLD;
        set_timeout(
            move || {
                self.lingering.update(|l| {
                    for x in l.iter_mut().filter(|x| x.row.id == id) {
                        x.phase = Phase::Exit;
                    }
                });
                set_timeout(
                    move || self.lingering.update(|l| l.retain(|x| x.row.id != id)),
                    Duration::from_millis(u64::from(motion::ROW_COLLAPSE)),
                );
            },
            Duration::from_millis(u64::from(hold)),
        );
        if undoable {
            self.show_toast(format!("Completed “{}”", row.title), Some(id));
        } else {
            // A recurring task: the next occurrence is already in the list, and reopening this one
            // would not remove it, so there is no Undo.
            self.show_toast(
                format!("Completed “{}”. The next one is scheduled", row.title),
                None,
            );
        }
    }

    fn uncomplete(self, id: TaskId) {
        self.lingering.update(|l| l.retain(|x| x.row.id != id));
        self.submit(Command::UncompleteTask { id });
    }

    fn toggle(self, row: &Row) {
        if row.done {
            self.uncomplete(row.id);
        } else {
            self.complete(row);
        }
    }

    /// Moves the selection by `delta` rows (J/K).
    fn step_selection(self, delta: isize) {
        let rows = self.shown.get_untracked();
        if rows.is_empty() {
            return;
        }
        let cur = self
            .selected
            .get_untracked()
            .and_then(|id| rows.iter().position(|s| s.row.id == id));
        let next = match cur {
            None => 0,
            Some(i) => i
                .saturating_add_signed(delta)
                .min(rows.len().saturating_sub(1)),
        };
        self.select(rows.get(next).map(|s| s.row.id));
    }
}

/// Starts the app: reads the backend into signals and mounts the page. `notice` is shown once, as a
/// toast (what the start had to repair).
pub fn mount(now: Now, live: bool, notice: Option<String>) {
    leptos::mount::mount_to_body(move || view! { <App now=now live=live notice=notice.clone()/> });
}

#[component]
fn App(now: Now, live: bool, notice: Option<String>) -> impl IntoView {
    let (room0, task0) = RoomId::from_path(&platform::path());
    let state = RwSignal::new(backend::view());
    let now = RwSignal::new(now);
    let room = RwSignal::new(room0);
    let lingering = RwSignal::new(Vec::<Linger>::new());
    let filter_text = RwSignal::new(String::new());
    let view_room = Memo::new(move |_| {
        filter_text.with(|f| state.with(|s| model::room(s, &now.get(), &room.get(), f)))
    });
    let shown = Memo::new(move |_| {
        let r = view_room.get();
        let mut rows: Vec<Shown> = r
            .rows
            .iter()
            .cloned()
            .map(|row| Shown { row, phase: None })
            .collect();
        if r.id != RoomId::Smart("completed") {
            for l in lingering.get().iter().filter(|l| {
                l.room == r.id && (!r.filtered || r.completed.iter().any(|row| row.id == l.row.id))
            }) {
                if rows.iter().any(|s| s.row.id == l.row.id) {
                    continue;
                }
                let at = l.index.min(rows.len());
                rows.insert(
                    at,
                    Shown {
                        row: l.row.clone(),
                        phase: Some(l.phase),
                    },
                );
            }
        }
        rows
    });
    let finished = Memo::new(move |_| {
        view_room.with(Room::finished)
            && lingering.with(|l| l.iter().all(|x| x.room != RoomId::default()))
    });
    let selected = RwSignal::new(task0);
    let detail = Memo::new(move |_| {
        selected
            .get()
            .and_then(|id| state.with(|s| model::detail(s, &now.get(), id)))
    });
    let sidebar = Memo::new(move |_| state.with(|s| model::sidebar(s, &now.get())));
    let completed_open = RwSignal::new(false);
    let columns = Memo::new(move |_| {
        let mut cols = shown.with(|rows| Columns::for_rows(rows.iter().map(|s| &s.row)));
        if completed_open.get() {
            view_room.with(|r| cols.include(&r.completed));
        }
        cols
    });
    let narrow = RwSignal::new(platform::detail_overlays());
    let detail_modal = Memo::new(move |_| narrow.get() && detail.with(Option::is_some));
    let c = Ctx {
        state,
        sync: RwSignal::new(backend::status()),
        synced_at: RwSignal::new(None),
        now,
        room,
        selected,
        lingering,
        toast: RwSignal::new(None),
        toast_seq: RwSignal::new(0),
        dark: RwSignal::new(platform::prefers_dark()),
        drawer: RwSignal::new(false),
        narrow,
        detail_modal,
        collapsed: RwSignal::new(BTreeSet::new()),
        new_list: RwSignal::new(false),
        filter_open: RwSignal::new(false),
        filter_text,
        completed_open,
        columns,
        editing: RwSignal::new(false),
        quick_add: NodeRef::new(),
        view_room,
        finished,
        shown,
        detail,
        sidebar,
    };
    provide_context(c);
    c.refresh();
    if let Some(n) = notice {
        c.show_toast(n, None);
    }

    // Theme and room hue live on <html>, where tokens.css looks for them.
    platform::on_media_change("(prefers-color-scheme: dark)", move |dark| c.dark.set(dark));
    platform::on_media_change("(max-width: 1199px)", move |n| c.narrow.set(n));
    platform::on_media_change("(max-width: 759px)", move |phone| {
        if !phone {
            c.drawer.set(false);
        }
    });
    Effect::new(move |_| {
        platform::set_root_attr("data-theme", if c.dark.get() { "dark" } else { "light" });
    });
    Effect::new(move |_| platform::set_root_attr("data-room-hue", c.view_room.get().hue));
    Effect::new(move |_| {
        let r = c.view_room.get();
        let title = if r.title.is_empty() {
            "To-do".to_owned()
        } else {
            format!("{} · To-do", r.title)
        };
        document().set_title(&title);
    });

    platform::on_pop_state(move || {
        let (r, t) = RoomId::from_path(&platform::path());
        if r != c.room.get_untracked() {
            c.reset_room_controls();
        }
        c.room.set(r);
        c.selected.set(t);
        c.editing.set(false);
    });

    let _keys = window_event_listener(leptos::ev::keydown, move |ev| on_key(c, &ev));

    if live {
        // Follow the sync engine, and keep "today" current.
        leptos::task::spawn_local(async move {
            loop {
                backend::next_change().await;
                c.refresh();
            }
        });
        set_interval(move || c.now.set(platform::now()), Duration::from_secs(30));
        platform::on_wake(backend::nudge);
    }

    track_overlay_focus(c);
    view! {
        <div class="app" class:has-detail=move || c.detail.with(Option::is_some)
            class:detail-modal=move || c.detail_modal.get() class:drawer-open=move || c.drawer.get()>
            <SidebarPane/>
            <div class="scrim" aria-hidden="true" on:click=move |_| {
                if c.drawer.get_untracked() { c.drawer.set(false); } else { c.select(None); }
            }></div>
            <main class="main" inert=move || c.drawer.get() || c.detail_modal.get()>
                <AppBar/>
                <RoomPane/>
                <ListFoot/>
            </main>
            <DetailPane/>
            <StatusBar/>
        </div>
    }
}

fn track_overlay_focus(c: Ctx) {
    Effect::new(move |prev: Option<(bool, Option<TaskId>, RoomId)>| {
        let modal = c.detail_modal.get();
        let id = c.detail.with(|d| d.as_ref().map(|d| d.id));
        let room = c.room.get();
        if modal && !prev.as_ref().is_some_and(|(open, _, _)| *open) {
            request_animation_frame(move || {
                if c.detail_modal.get_untracked() {
                    platform::focus("task-detail");
                }
            });
        } else if let Some((_, Some(old), old_room)) = prev
            && id.is_none()
            && room == old_room
        {
            request_animation_frame(move || {
                if c.selected.get_untracked().is_none() && !platform::focus(&format!("row-{old}")) {
                    platform::focus("room-title");
                }
            });
        }
        (modal, id, room)
    });
    Effect::new(move |prev| {
        let open = c.drawer.get();
        if open || prev == Some(true) {
            request_animation_frame(move || {
                if open == c.drawer.get_untracked() {
                    platform::focus(if open { "list-drawer" } else { "open-drawer" });
                }
            });
        }
        open
    });
}

fn typing(ev: &leptos::ev::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| {
            matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || el.has_attribute("contenteditable")
        })
}

fn on_key(c: Ctx, ev: &leptos::ev::KeyboardEvent) {
    use wasm_bindgen::JsCast;
    if ev.is_composing() || ev.default_prevented() {
        return;
    }
    let key = ev.key().to_ascii_lowercase();
    let drawer = c.drawer.get_untracked();
    let modal = c.detail_modal.get_untracked();
    if key == "tab" && (drawer || modal) {
        platform::contain_tab(if drawer { "list-drawer" } else { "task-detail" }, ev);
        return;
    }
    if key == "escape" {
        if c.editing.get_untracked() {
            c.editing.set(false);
        } else if c.new_list.get_untracked() && !modal {
            c.new_list.set(false);
            platform::focus("open-new-list");
        } else if drawer {
            c.drawer.set(false);
        } else if c.selected.get_untracked().is_some() {
            c.select(None);
        } else if c.filter_open.get_untracked() {
            c.close_filter();
        }
        return;
    }
    let typing = typing(ev);
    if !drawer
        && !modal
        && !ev.alt_key()
        && !ev.meta_key()
        && ((ev.ctrl_key() && key == "k") || (!typing && !ev.ctrl_key() && key == "/"))
    {
        ev.prevent_default();
        c.open_filter();
        return;
    }
    if typing || drawer || ev.alt_key() || ev.meta_key() {
        return;
    }
    if ev.ctrl_key() {
        if key == "z"
            && let Some(id) = c.toast.get_untracked().and_then(|t| t.undo)
        {
            ev.prevent_default();
            c.toast.set(None);
            c.uncomplete(id);
        }
        return;
    }
    match key.as_str() {
        "j" | "arrowdown" => {
            ev.prevent_default();
            c.step_selection(1);
        }
        "k" | "arrowup" => {
            ev.prevent_default();
            c.step_selection(-1);
        }
        " " if !c.read_only() => {
            let selected = c.selected.get_untracked();
            // Keep native activation on other controls, including unselected row titles.
            if ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|el| {
                    matches!(el.tag_name().as_str(), "BUTTON" | "A")
                        && !selected.is_some_and(|id| el.id() == format!("row-{id}"))
                })
            {
                return;
            }
            let row = selected.and_then(|id| {
                c.shown
                    .get_untracked()
                    .into_iter()
                    .find(|s| s.row.id == id && s.phase.is_none())
                    .map(|s| s.row)
            });
            if let Some(row) = row {
                ev.prevent_default();
                c.toggle(&row);
            }
        }
        "e" => {
            if c.detail.with_untracked(Option::is_some) && !c.read_only() {
                ev.prevent_default();
                c.editing.set(true);
            }
        }
        "n" if !modal => {
            if let Some(i) = c.quick_add.get_untracked().filter(|i| !i.disabled()) {
                ev.prevent_default();
                let _ = i.focus();
            }
        }
        _ => {}
    }
}

// ---- small pieces -------------------------------------------------------------------------------------

fn icon(i: Icon) -> impl IntoView {
    view! { <span class="icon" aria-hidden="true" inner_html=i.svg()></span> }
}

fn glyph(g: Glyph) -> impl IntoView {
    view! { <span class="glyph" aria-hidden="true" inner_html=g.svg()></span> }
}

fn mark(m: &AgentMark) -> impl IntoView + use<> {
    let name = format!("Agent: {}", model::agent_display(&m.name));
    view! {
        <span class="mark" data-agent=m.key.unwrap_or("other") title=name.clone() aria-label=name role="img">
            {m.letter.clone()}
        </span>
    }
}

fn you_mark() -> impl IntoView {
    view! { <span class="mark you" title="You" aria-label="You" role="img">"Y"</span> }
}

fn chip(label: String, hue: &'static str) -> impl IntoView {
    let full_label = label.clone();
    view! {
        <span class="chip" data-list-hue=hue title=full_label>
            <i class="dot"></i><span class="chip-label">{label}</span>
        </span>
    }
}

fn bars(done: usize, total: usize) -> impl IntoView {
    let n = total.clamp(1, 5);
    let filled = (done * n + total / 2).checked_div(total).unwrap_or(0);
    let filled = if done > 0 { filled.max(1) } else { filled };
    view! {
        <span class="bars" aria-hidden="true">
            {(0..n).map(|i| view! { <i class:on=i < filled></i> }).collect_view()}
        </span>
    }
}

fn tick_svg() -> impl IntoView {
    view! {
        <svg class="tick" viewBox="0 0 20 20" aria-hidden="true" focusable="false">
            <path d="M5.6 10.4l3 3 5.8-6.6" pathLength="1"></path>
        </svg>
    }
}

fn link_room(c: Ctx, room: RoomId) -> impl Fn(leptos::ev::MouseEvent) + Clone + 'static {
    move |ev: leptos::ev::MouseEvent| {
        if ev.ctrl_key() || ev.meta_key() || ev.shift_key() || ev.button() != 0 {
            return;
        }
        ev.prevent_default();
        c.go(room.clone());
    }
}

fn sync_text(s: &Sync, at: Option<Now>) -> String {
    let mut t = if s.signed_out {
        "Signed out".to_owned()
    } else {
        match s.connection {
            Connection::Offline if s.pending > 0 => format!("Offline · {} pending", s.pending),
            Connection::Offline => "Offline".to_owned(),
            Connection::Syncing => "Syncing…".to_owned(),
            Connection::Online => at.map_or_else(
                || "Synced".to_owned(),
                |n| {
                    let l = n.local();
                    format!("Synced {:02}:{:02}", l.hour(), l.minute())
                },
            ),
        }
    };
    if s.failed > 0 {
        t.push_str(&format!(" · {} not synced", s.failed));
    }
    t
}

fn conn_key(s: &Sync) -> &'static str {
    match s.connection {
        Connection::Online => "online",
        Connection::Syncing => "syncing",
        Connection::Offline => "offline",
    }
}

// ---- sidebar ------------------------------------------------------------------------------------------

#[component]
fn SidebarPane() -> impl IntoView {
    let c = ctx();
    let item = move |n: model::NavItem, sub: bool, children: Vec<RoomId>| {
        let room = n.room.clone();
        let active_room = n.room.clone();
        let active = move || c.room.with(|r| *r == active_room);
        let group_room = n.room.clone();
        let contains_active = move || match &group_room {
            RoomId::Collection(id) if c.collapsed.with(|ids| ids.contains(id)) => {
                c.room.with(|r| children.contains(r))
            }
            _ => false,
        };
        let href = room.path();
        let lead = match (&n.room, n.glyph) {
            (RoomId::Smart(id), _) => icon(model::smart_icon(id)).into_any(),
            (_, Some(g)) => glyph(g).into_any(),
            _ => ().into_any(),
        };
        view! {
            <a
                class="nav-item"
                class:sub=sub
                class:active=active.clone()
                class:contains-active=contains_active
                aria-current=move || active().then_some("page")
                href=href
                data-list-hue=n.hue
                on:click=link_room(c, room)
            >
                {lead}
                <span class="nav-name">{n.name.clone()}</span>
                <span class="count">{(n.count > 0).then(|| n.count.to_string())}</span>
            </a>
        }
    };
    view! {
        <aside class="sidebar" id="list-drawer" tabindex="-1" aria-label="Lists"
            role=move || c.drawer.get().then_some("dialog")
            aria-modal=move || c.drawer.get().then_some("true")
            inert=move || c.detail_modal.get()
        >
            <div class="brand">
                <img class="brand-mark" src="/app/brand/icon-small.svg" alt="" />
                <span>"To-do"</span>
            </div>
            <nav class="nav">
                {move || {
                    let sb = c.sidebar.get();
                    view! {
                        <div class="nav-section">
                            {sb.smart.into_iter().map(|n| item(n, false, Vec::new())).collect_view()}
                        </div>
                        {sb
                            .groups
                            .into_iter()
                            .filter_map(|g| {
                                let RoomId::Collection(id) = g.head.room else { return None };
                                let label = g.head.name.clone();
                                let control = format!("collection-{id}");
                                let children = g.lists.iter().map(|n| n.room.clone()).collect();
                                let folded = move || c.collapsed.with(|ids| ids.contains(&id));
                                Some(view! {
                                    <div class="nav-section">
                                        <div class="nav-collection-row">
                                            <button type="button" class="icon-button collection-toggle"
                                                aria-expanded=move || (!folded()).to_string()
                                                aria-controls=control.clone()
                                                aria-label=move || format!("{} {label}", if folded() { "Expand" } else { "Collapse" })
                                                on:click=move |_| c.collapsed.update(|ids| { if !ids.remove(&id) { ids.insert(id); } })
                                            >{icon(Icon::ChevronRight)}</button>
                                            {item(g.head, false, children)}
                                        </div>
                                        <div id=control hidden=folded>
                                            {g.lists.into_iter().map(|n| item(n, true, Vec::new())).collect_view()}
                                        </div>
                                    </div>
                                })
                            })
                            .collect_view()}
                        {(!sb.loose.is_empty())
                            .then(|| {
                                view! {
                                    <div class="nav-section">
                                        <div class="caps nav-caps">"Lists"</div>
                                        {sb.loose.into_iter().map(|n| item(n, false, Vec::new())).collect_view()}
                                    </div>
                                }
                            })}
                        {(!sb.agents.is_empty()).then(|| view! {
                            <section class="nav-section side-agents" aria-label="Agents">
                                <h2 class="caps nav-caps">"Agents"</h2>
                                <ul>
                                    {sb.agents.into_iter().map(|(agent, status)| view! {
                                        <li>{mark(&agent)}<span>{model::agent_display(&agent.name)}</span><span class="count">{status}</span></li>
                                    }).collect_view()}
                                </ul>
                            </section>
                        })}
                    }
                }}
            </nav>
            <NewList/>
            <div class="side-foot">
                <span class="conn" data-conn=move || conn_key(&c.sync.get())></span>
                <span class="sync-text">{move || sync_text(&c.sync.get(), c.synced_at.get())}</span>
                <SignOut/>
            </div>
        </aside>
    }
}

#[component]
fn NewList() -> impl IntoView {
    let c = ctx();
    let close = move || {
        c.new_list.set(false);
        platform::focus("open-new-list");
    };
    let input = NodeRef::<html::Input>::new();
    view! {
        <div class="new-list">
            <button type="button" id="open-new-list" class="button open-new-list"
                aria-expanded=move || c.new_list.get().to_string()
                disabled=move || c.read_only()
                on:click=move |_| {
                    if c.new_list.get_untracked() { close(); } else {
                        c.new_list.set(true);
                        request_animation_frame(|| { platform::focus("new-list-name"); });
                    }
                }
            >{icon(Icon::Plus)}"New list"</button>
            <Show when=move || c.new_list.get()>
                <form on:submit=move |ev| {
                    ev.prevent_default();
                    let Some(i) = input.get_untracked() else { return };
                    let name = i.value().trim().to_owned();
                    if name.is_empty() { return; }
                    let collection = match c.room.get_untracked() {
                        RoomId::Collection(id) => Some(id),
                        RoomId::List(id) => c.state.with_untracked(|s| s.lists.get(&id).and_then(|l| l.collection)),
                        RoomId::Smart(_) => None,
                    };
                    let id = ListId::from_uuid(backend::new_uuid());
                    if c.submit(Command::CreateList { id, collection, name, color: None, placement: Placement::Last }) {
                        if let Some(group) = collection {
                            c.collapsed.update(|ids| { ids.remove(&group); });
                        }
                        c.go(RoomId::List(id));
                        request_animation_frame(|| { platform::focus("room-title"); });
                    }
                }>
                    <input id="new-list-name" node_ref=input type="text" autocomplete="off" required
                        aria-label="List name" placeholder="List name…" disabled=move || c.read_only()/>
                    <button class="icon-button" type="submit" aria-label="Create list" title="Create list"
                        disabled=move || c.read_only()>{icon(Icon::Plus)}</button>
                    <button class="icon-button" type="button" aria-label="Cancel new list" title="Cancel"
                        on:click=move |_| close()>{icon(Icon::X)}</button>
                </form>
            </Show>
        </div>
    }
}

#[component]
fn SignOut() -> impl IntoView {
    let csrf = backend::csrf();
    let field = NodeRef::<html::Input>::new();
    csrf.map(|token| {
        view! {
            <form
                class="sign-out"
                method="post"
                action="/auth/logout"
                on:submit=move |_| {
                    // The token may have changed since the page loaded (see `backend::csrf`).
                    if let (Some(i), Some(t)) = (field.get(), backend::csrf()) {
                        i.set_value(&t);
                    }
                }
            >
                <input type="hidden" name="csrf" value=token node_ref=field/>
                <button type="submit" class="icon-button" title="Sign out" aria-label="Sign out">
                    {icon(Icon::LogOut)}
                </button>
            </form>
        }
    })
}

// ---- phone app bar and room pills ------------------------------------------------------------------------

#[component]
fn AppBar() -> impl IntoView {
    let c = ctx();
    view! {
        <div class="app-bar" class:filter-open=move || c.filter_open.get()
            data-theme=move || c.finished.get().then_some("dark")
            data-room-hue=move || c.finished.get().then_some("my-day")>
            <button type="button" class="icon-button drawer-button" id="open-drawer" aria-label="Lists"
                on:click=move |_| c.drawer.set(true)>{icon(Icon::Menu)}</button>
            <span class="app-bar-sync" data-conn=move || conn_key(&c.sync.get())>
                {move || {
                    let s = c.sync.get();
                    (s.connection != Connection::Online || s.pending > 0)
                        .then(|| sync_text(&s, c.synced_at.get()))
                }}
            </span>
            <button type="button" class="filter-trigger" id="open-filter" aria-label="Filter tasks"
                aria-keyshortcuts="/ Control+K" title="Filter tasks (/ or Ctrl+K)"
                aria-expanded=move || c.filter_open.get().to_string()
                on:click=move |_| c.open_filter()
            >{icon(Icon::Search)}<span>"Filter"</span><kbd class="keycap">"Ctrl K"</kbd></button>
            <Show when=move || c.filter_open.get()>
                <div class="filter-field" role="search">
                    {icon(Icon::Search)}
                    <input type="search" id="room-filter" aria-label="Filter tasks"
                        autocomplete="off" placeholder="Filter tasks… e.g. tag:docs"
                        prop:value=move || c.filter_text.get()
                        on:input=move |ev| c.filter_text.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Escape" && !ev.is_composing() {
                                ev.prevent_default();
                                ev.stop_propagation();
                                c.close_filter();
                            }
                        }
                    />
                    <button type="button" class="icon-button" aria-label="Close filter" title="Close filter (Esc)"
                        on:click=move |_| c.close_filter()>{icon(Icon::X)}</button>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn RoomPills() -> impl IntoView {
    let c = ctx();
    view! {
        <nav class="pills" aria-label="Rooms">
            {move || {
                let sb = c.sidebar.get();
                let smart = sb
                    .smart
                    .into_iter()
                    .filter(|n| matches!(n.room, RoomId::Smart("my_day" | "important" | "planned")));
                let colls = sb.groups.into_iter().map(|g| g.head);
                smart
                    .chain(colls)
                    .chain(sb.loose)
                    .map(|n| {
                        let room = n.room.clone();
                        let active_room = n.room.clone();
                        let lead = match (&n.room, n.glyph) {
                            (RoomId::Smart(id), _) => icon(model::smart_icon(id)).into_any(),
                            (_, Some(g)) => glyph(g).into_any(),
                            _ => ().into_any(),
                        };
                        view! {
                            <a
                                class="pill"
                                class:active=move || c.room.get() == active_room
                                href=n.room.path()
                                data-list-hue=n.hue
                                on:click=link_room(c, room)
                            >
                                {lead}
                                {n.name.clone()}
                            </a>
                        }
                    })
                    .collect_view()
            }}
        </nav>
    }
}

// ---- the room ----------------------------------------------------------------------------------------------

#[component]
fn RoomPane() -> impl IntoView {
    let c = ctx();
    let finished = c.finished;
    let node = NodeRef::<html::Div>::new();
    let width = RwSignal::new(0.0_f64);
    node.on_load(move |el| {
        width.set(f64::from(el.client_width()));
        platform::observe_width(&el, move |w| width.set(w));
    });
    Effect::new(move |prev: Option<RoomId>| {
        let room = c.room.get();
        if prev.is_some_and(|old| old != room)
            && let Some(el) = node.get_untracked()
        {
            el.set_scroll_top(0);
        }
        room
    });
    view! {
        <div class="room" node_ref=node>
            <Show when=move || finished.get() fallback=move || view! { <RoomHeader width=width/> }>
                <FinishedHeader width=width/>
            </Show>
            <RoomPills/>
            <QuickAdd/>
            <Show when=move || finished.get() && !c.view_room.with(|r| r.filtered) fallback=|| view! { <TaskList/> }>
                <Handover/>
            </Show>
            <CompletedRows/>
        </div>
    }
}

#[component]
fn RoomHeader(width: RwSignal<f64>) -> impl IntoView {
    let c = ctx();
    // The sun and the count wait for completing rows to leave.
    let leaving = Memo::new(move |_| {
        let id = c.view_room.with(|r| r.id.clone());
        c.lingering
            .with(|l| l.iter().filter(|x| x.room == id).count())
    });
    let counts = Memo::new(move |_| {
        let r = c.view_room.get();
        let done = r.done.saturating_sub(leaving.get());
        (done, r.total)
    });
    view! {
        <header class="room-head">
            <div class="head-text">
                <div class="eyebrow">
                    {move || {
                        let e = c.view_room.with(|r| r.eyebrow.clone());
                        view! {
                            <span class="eyebrow-lead">{e.lead}</span>
                            {e.meta.into_iter().map(|m| view! { <span class="eyebrow-meta">{m}</span> }).collect_view()}
                        }
                    }}
                </div>
                <div class="title-row">
                    <RoomTitle/>
                    <p class="summary">
                        {move || {
                            let (done, total) = counts.get();
                            if total == 0 {
                                // An empty room keeps score of nothing.
                                ().into_any()
                            } else if c.view_room.with(|r| r.id == RoomId::Smart("completed")) {
                                view! { <b>{total}</b>" done" }.into_any()
                            } else {
                                view! {
                                    <b>{done}</b>
                                    {format!(" of {total} done")}
                                    <span class="dot-sep">"·"</span>
                                    <span class="fig">{total - done}</span>
                                    " open"
                                }
                                    .into_any()
                            }
                        }}
                    </p>
                </div>
            </div>
            <LandscapeStrip counts=counts width=width/>
        </header>
    }
}

#[component]
fn RoomTitle() -> impl IntoView {
    let c = ctx();
    view! { <h1 id="room-title" class="room-title" tabindex="-1">{move || c.view_room.with(|r| r.title.clone())}</h1> }
}

/// The slim landscape, measured to its width. The sun eases along the arc when the count changes.
#[component]
fn LandscapeStrip(counts: Memo<(usize, usize)>, width: RwSignal<f64>) -> impl IntoView {
    let c = ctx();
    let target = Memo::new(move |_| {
        let (d, t) = counts.get();
        Landscape::progress(d, t)
    });
    let sun = RwSignal::new(target.get_untracked());
    Effect::new(move |prev: Option<f32>| {
        let to = target.get();
        if let Some(from) = prev
            && (to - from).abs() > f32::EPSILON
            && !platform::reduced_motion()
        {
            let start = platform::clock_ms();
            animate_sun(sun, sun.get_untracked(), to, start);
        } else {
            sun.set(to);
        }
        to
    });
    let markup = move || {
        let w = width.get();
        if w <= 0.0 {
            return String::new();
        }
        let height = if platform::is_phone() {
            HEIGHT_COVER
        } else {
            HEIGHT_DESKTOP
        };
        #[allow(clippy::cast_possible_truncation)]
        landscape::svg(
            w as f32,
            Sky::Slim {
                height,
                night: c.dark.get(),
            },
            sun.get(),
            counts.get().1,
        )
    };
    view! { <div class="landscape" inner_html=markup></div> }
}

fn animate_sun(sun: RwSignal<f32>, from: f32, to: f32, start: f64) {
    request_animation_frame(move || {
        #[allow(clippy::cast_possible_truncation)]
        let t =
            ((platform::clock_ms() - start) / f64::from(motion::SUN_TRAVEL)).clamp(0.0, 1.0) as f32;
        let e = motion::SUN.ease(t);
        let next = from + (to - from) * e;
        if sun.try_update(|value| *value = next).is_some() && t < 1.0 {
            animate_sun(sun, from, to, start);
        }
    });
}

#[component]
fn QuickAdd() -> impl IntoView {
    let c = ctx();
    let value = RwSignal::new(String::new());
    let add = move || {
        let title = value.get_untracked().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let Some(target) = c.view_room.with_untracked(|r| r.add.clone()) else {
            return;
        };
        let today = c.now.get_untracked().today();
        let mut t = NewTask::new(TaskId::from_uuid(backend::new_uuid()), target.list, title);
        t.placement = Placement::Last;
        if target.my_day {
            t.my_day = Some(today);
        }
        if target.important {
            t.priority = Priority::High;
        }
        if target.due_today {
            t.due = Some(today);
        }
        if c.submit(Command::CreateTask(Box::new(t))) {
            value.set(String::new());
        }
    };
    let add_target = Memo::new(move |_| c.view_room.with(|r| r.add.clone()));
    view! {
        {move || {
            add_target.get().map(|target| {
                    view! {
                        <form class="quick-add" on:submit=move |ev| { ev.prevent_default(); add(); }>
                            <span class="qa-plus">{icon(Icon::Plus)}</span>
                            <input
                                node_ref=c.quick_add
                                class="qa-input"
                                type="text"
                                disabled=move || c.read_only()
                                autocomplete="off"
                                enterkeyhint="done"
                                aria-label="Add a task"
                                placeholder=target.placeholder
                                prop:value=move || value.get()
                                on:input=move |ev| value.set(event_target_value(&ev))
                            />
                            <span class="qa-hints" aria-hidden="true" hidden=move || c.read_only()>
                                <span class="keycap">"N"</span>
                            </span>
                        </form>
                    }
                })
        }}
    }
}

#[component]
fn TaskList() -> impl IntoView {
    let c = ctx();
    let empty = Memo::new(move |_| {
        c.shown.with(Vec::is_empty) && c.view_room.with(|r| !r.filtered || r.completed.is_empty())
    });
    view! {
        <Show when=move || empty.get()>
            <p class="empty-line">{move || c.view_room.with(Room::empty_line)}</p>
        </Show>
        <ul class="rows" role="list">
            <For each=move || c.shown.get() key=|s| s.clone() let:s>
                <TaskRow row=s.row phase=s.phase/>
            </For>
        </ul>
    }
}

#[component]
fn CompletedRows() -> impl IntoView {
    let c = ctx();
    let completed = Memo::new(move |_| {
        c.view_room.with(|r| {
            r.completed
                .iter()
                .filter(|row| {
                    !c.lingering
                        .with(|l| l.iter().any(|x| x.room == r.id && x.row.id == row.id))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    view! {
        <ul id="completed-rows" class="rows completed-rows" role="list" aria-label="Completed tasks"
            hidden=move || !c.completed_open.get() || completed.with(Vec::is_empty)>
            <For each=move || if c.completed_open.get() { completed.get() } else { Vec::new() }
                key=|r| r.clone() let:row>
                <TaskRow row=row phase=None/>
            </For>
        </ul>
    }
}

#[component]
fn ListFoot() -> impl IntoView {
    let c = ctx();
    let count = Memo::new(move |_| c.view_room.with(|r| r.completed.len()));
    let has_completed = move || count.get() > 0;
    view! {
        <div class="list-foot">
            <Show when=has_completed>
                <button type="button" class="completed-toggle"
                    class:open=move || c.completed_open.get()
                    aria-expanded=move || c.completed_open.get().to_string()
                    aria-controls="completed-rows"
                    on:click=move |_| {
                        let open = !c.completed_open.get_untracked();
                        c.completed_open.set(open);
                        if open {
                            request_animation_frame(|| {
                                if let Some(rows) = document().get_element_by_id("completed-rows") {
                                    rows.scroll_into_view();
                                }
                            });
                        }
                    }
                >
                    {icon(Icon::ChevronRight)}
                    <span class="caps">"Completed"</span>
                    <span class="count">{move || count.get()}</span>
                </button>
            </Show>
            <ToastView/>
        </div>
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Columns {
    agent: bool,
    priority: bool,
    due: bool,
}

impl Columns {
    fn for_rows<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Self {
        let mut columns = Self::default();
        columns.include(rows);
        columns
    }

    fn include<'a>(&mut self, rows: impl IntoIterator<Item = &'a Row>) {
        for row in rows {
            self.agent |= row.agent.is_some();
            self.priority |= row.priority > 0;
            self.due |= row.due.is_some();
        }
    }
}

#[component]
fn TaskRow(row: Row, phase: Option<Phase>) -> impl IntoView {
    let c = ctx();
    let id = row.id;
    let r = row.clone();
    let on_check = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        if phase.is_none() {
            c.toggle(&r);
        }
    };
    let overdue = row.due.as_ref().is_some_and(|d| d.tone == DueTone::Overdue);
    let due_view = |cls: &'static str| {
        row.due.clone().map(|d| {
            let tone = match d.tone {
                DueTone::Overdue => "overdue",
                DueTone::Soon => "soon",
                DueTone::Later => "later",
            };
            view! {
                <span class=cls data-tone=tone>
                    {(d.tone == DueTone::Overdue).then(|| view! { <i class="diamond" aria-hidden="true"></i> })}
                    {d.text}
                </span>
            }
        })
    };
    let tags = row
        .tags
        .iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join("  ");
    let full_tags = tags.clone();
    let label = format!(
        "{} “{}”",
        if row.done { "Reopen" } else { "Complete" },
        row.title
    );
    view! {
        <li
            class="row"
            class:done=row.done
            class:completing=phase == Some(Phase::Hold)
            class:exiting=phase == Some(Phase::Exit)
            class:overdue=overdue
            class:selected=move || c.selected.get() == Some(id)
            class:with-agent=move || c.columns.with(|cols| cols.agent)
            class:with-priority=move || c.columns.with(|cols| cols.priority)
            class:with-due=move || c.columns.with(|cols| cols.due)
            on:click=move |_| c.select(Some(id))
        >
            <button
                class="check"
                role="checkbox"
                disabled=move || c.read_only()
                aria-checked=row.done.to_string()
                aria-label=label
                on:click=on_check
            >
                {tick_svg()}
            </button>
            <div class="row-main">
                <button type="button" class="row-title" id=format!("row-{id}")>{row.title.clone()}</button>
                <span class="meta">
                    <span class="due-slot">{due_view("due")}</span>
                    {row.chip.clone().map(|ch| chip(ch.label, ch.hue))}
                    {row.steps.map(|(d, t)| view! { <span class="steps">{bars(d, t)}{format!("{d}/{t}")}</span> })}
                    {row.email.then(|| view! { <span class="meta-icon" title="From an email" aria-label="From an email" role="img">{icon(Icon::Mail)}</span> })}
                    {(!tags.is_empty()).then(|| view! { <span class="tag" title=full_tags>{tags}</span> })}
                </span>
            </div>
            <span class="col-agent">{row.agent.as_ref().map(mark)}</span>
            <span class="col-priority" data-p=row.priority.to_string()
                aria-label=(row.priority > 0).then(|| format!("Priority {}", row.priority))>
                {"!".repeat(usize::from(row.priority))}
            </span>
            <span class="col-due">{due_view("due")}</span>
        </li>
    }
}

#[component]
fn ToastView() -> impl IntoView {
    let c = ctx();
    view! {
        {move || {
            // Another tab waits to upgrade until this one's changes are stored.
            if c.sync.with(|s| s.holding_upgrade) {
                let e = c.sync.with(|s| s.local_error.clone()).unwrap_or_default();
                return Some(
                    view! {
                        <div class="toast toast-error toast-holding" role="alert" title=format!("The browser said: {e}")>
                            <span class="toast-text toast-long">
                                "This tab has changes it couldn’t save yet; other tabs will update once they’re saved."
                            </span>
                            <span class="toast-text toast-short">"Saving before other tabs update"</span>
                        </div>
                    }
                    .into_any(),
                );
            }
            // A newer version took the database over: no more edits until the tab reloads.
            if c.read_only() {
                return Some(
                    view! {
                        <div class="toast toast-error toast-read-only" role="alert">
                            <span class="toast-text toast-long">
                                "A newer version is open in another tab. Reload to keep editing"
                            </span>
                            <span class="toast-text toast-short">"Newer version open"</span>
                            <button class="toast-undo" on:click=move |_| platform::reload()>"Reload"</button>
                        </div>
                    }
                    .into_any(),
                );
            }
            // A change this browser couldn't store stays on screen until storing works again.
            if let Some(e) = c.sync.with(|s| s.local_error.clone()) {
                return Some(
                    view! {
                        <div class="toast toast-error" role="alert" title=format!("Retrying. The browser said: {e}")>
                            <span class="toast-text toast-long">"Couldn’t save in this browser. Retrying; keep this tab open."</span>
                            <span class="toast-text toast-short">"Not saved · keep this tab open"</span>
                        </div>
                    }
                    .into_any(),
                );
            }
            c.toast
                .get()
                .map(|t| {
                    let undo = t.undo;
                    view! {
                        <div class="toast" role="status">
                            <span class="toast-text">{t.text}</span>
                            {undo
                                .map(|id| {
                                    view! {
                                        <button
                                            class="toast-undo"
                                            on:click=move |_| {
                                                c.toast.set(None);
                                                c.uncomplete(id);
                                            }
                                        >
                                            {icon(Icon::UndoTwo)}
                                            "Undo"
                                            <span class="keycap">"Ctrl Z"</span>
                                        </button>
                                    }
                                })}
                            <button class="toast-close" aria-label="Dismiss" on:click=move |_| c.toast.set(None)>
                                {icon(Icon::X)}
                            </button>
                            <i class="toast-timer" aria-hidden="true"></i>
                        </div>
                    }
                    .into_any()
                })
        }}
    }
}

// ---- the finished day ------------------------------------------------------------------------------------

#[component]
fn FinishedHeader(width: RwSignal<f64>) -> impl IntoView {
    let c = ctx();
    view! {
        <header class="room-head finished" data-theme="dark" data-room-hue="my-day">
            <div class="head-text">
                <div class="eyebrow">
                    {move || {
                        let e = c.view_room.with(|r| r.eyebrow.clone());
                        view! {
                            <span class="eyebrow-lead">{e.lead}</span>
                            {e.meta.into_iter().map(|m| view! { <span class="eyebrow-meta">{m}</span> }).collect_view()}
                        }
                    }}
                </div>
                <div class="title-row">
                    <RoomTitle/>
                </div>
            </div>
            <div
                class="landscape tall"
                inner_html=move || {
                    let w = width.get();
                    #[allow(clippy::cast_possible_truncation)]
                    if w > 0.0 {
                        landscape::svg(w as f32, Sky::Finished, 1.0, 1)
                    } else {
                        String::new()
                    }
                }
            ></div>
        </header>
    }
}

#[component]
fn Handover() -> impl IntoView {
    let c = ctx();
    let tomorrow = Memo::new(move |_| c.state.with(|s| model::tomorrow(s, &c.now.get())));
    let small_row = |r: Row, date: Option<String>| {
        view! {
            <li class="t-row">
                <span class="check static" aria-hidden="true"></span>
                <span class="t-title">{r.title}</span>
                {r.chip.map(|ch| chip(ch.label, ch.hue))}
                <span class="t-date">{date}</span>
            </li>
        }
    };
    view! {
        <section class="tomorrow">
            {move || {
                let t = tomorrow.get();
                let weekday = t.weekday.clone();
                view! {
                    <h2 class="caps">"When the sun comes up"</h2>
                    <div class="next">
                        <span class="sunrise">{icon(Icon::Sunrise)}</span>
                        {if let Some((r, date)) = t.first {
                            view! {
                                <div class="next-text">
                                    <div class="next-title">{r.title}</div>
                                    <div class="next-meta">
                                        <span class="due" data-tone="later">{date}</span>
                                        {r.chip.map(|ch| chip(ch.label, ch.hue))}
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <p class="next-text no-tomorrow">"Nothing planned for tomorrow."</p> }.into_any()
                        }}
                        <a class="button primary" href=RoomId::Smart("planned").path() on:click=link_room(c, RoomId::Smart("planned"))>
                            {format!("Plan {weekday}")}
                        </a>
                    </div>
                    {(!t.also.is_empty())
                        .then(|| {
                            view! {
                                <h2 class="caps">{format!("Also on {}", t.weekday)}</h2>
                                <ul class="t-rows">
                                    {t.also.clone().into_iter().map(|r| small_row(r, None)).collect_view()}
                                </ul>
                            }
                        })}
                    {(!t.week.is_empty())
                        .then(|| {
                            let today = c.now.get_untracked().today();
                            let state = c.state.get_untracked();
                            view! {
                                <h2 class="caps">"Rest of the week"</h2>
                                <ul class="t-rows">
                                    {t
                                        .week
                                        .clone()
                                        .into_iter()
                                        .map(|r| {
                                            let date = state
                                                .tasks
                                                .get(&r.id)
                                                .and_then(|x| x.due)
                                                .map(|d| model::short_date(d, today));
                                            small_row(r, date)
                                        })
                                        .collect_view()}
                                </ul>
                                {(t.more > 0).then(|| view! { <p class="more">{format!("{} more", t.more)}</p> })}
                            }
                        })}
                }
            }}
        </section>
    }
}

// ---- detail ------------------------------------------------------------------------------------------------

#[component]
fn DetailPane() -> impl IntoView {
    let c = ctx();
    // Mounted once per task: a live update of the same task changes the fields in place, so an
    // editor being typed in keeps its draft (and its focus).
    // Keep the last body behind an inert, hidden panel for its CSS exit transition.
    let rendered = Memo::new(move |prev: Option<&Option<Detail>>| {
        c.detail.get().or_else(|| prev.cloned().flatten())
    });
    let id = Memo::new(move |_| rendered.with(|d| d.as_ref().map(|d| d.id)));
    view! {
        <section class="detail" id="task-detail" tabindex="-1" aria-label="Task detail"
            role=move || c.detail_modal.get().then_some("dialog")
            aria-modal=move || c.detail_modal.get().then_some("true")
            inert=move || c.detail.with(Option::is_none)>
            {move || id.get().map(|id| view! { <DetailBody id=id rendered=rendered/> })}
        </section>
    }
}

#[component]
fn DetailBody(id: TaskId, rendered: Memo<Option<Detail>>) -> impl IntoView {
    let d = Memo::new(move |_| rendered.get().filter(|d| d.id == id));
    let field = move |f: fn(&Detail) -> String| {
        Signal::derive(move || d.with(|d| d.as_ref().map(f).unwrap_or_default()))
    };
    let title = field(|d| d.title.clone());
    let notes = field(|d| d.notes.clone());
    let done = move || d.with(|d| d.as_ref().is_some_and(|d| d.done));
    view! {
        <DetailTop id=id detail=d/>
        <div class="detail-body">
            <div class="d-head" class:done=done>
                {move || d.get().map(|d| view! { <DetailCheck d=d/> })}
                <TitleEditor id=id title=title/>
            </div>
            {move || d.get().map(|d| view! { <p class="stamps">{d.stamps.clone()}</p> })}

            <section class="d-section">
                {move || d.get().map(|d| view! { <DetailSteps d=d/> })}
                <AddStep id=id/>
            </section>

            <section class="d-section">
                <h3 class="caps d-caps">"Notes"</h3>
                <NotesEditor id=id notes=notes/>
            </section>

            {move || d.get().map(|d| view! { <DetailRest d=d/> })}
        </div>
    }
}

#[component]
fn DetailTop(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let important = move || detail.with(|d| d.as_ref().is_some_and(|d| d.important));
    let toggle_important = move |_| {
        let p = if important() {
            Priority::None
        } else {
            Priority::High
        };
        c.submit(Command::UpdateTask {
            id,
            patch: TaskPatch {
                priority: Some(p),
                ..TaskPatch::default()
            },
        });
    };
    let my_day = move || detail.with(|d| d.as_ref().is_some_and(|d| d.my_day));
    let toggle_my_day = move |_| {
        let date = (!my_day()).then(|| c.now.get_untracked().today());
        c.submit(Command::SetMyDay { id, date });
    };
    view! {
        <div class="detail-top">
            <div class="crumb">
                {move || detail.with(|d| d.as_ref().map(|d| d.crumb.clone())).map(|(group, list)| {
                    let full_name = list.clone();
                    view! {
                        {group.map(|ch| view! { {chip(ch.label, ch.hue)}<span class="crumb-sep">"/"</span> })}
                        <span class="crumb-list" title=full_name>{list}</span>
                    }
                })}
            </div>
            <button
                class="icon-button"
                class:on=my_day
                aria-pressed=move || my_day().to_string()
                title=move || if my_day() { "Remove from My Day" } else { "Add to My Day" }
                aria-label="My Day"
                disabled=move || c.read_only()
                on:click=toggle_my_day
            >
                {icon(Icon::Sun)}
            </button>
            <button
                class="icon-button"
                class:on=important
                aria-pressed=move || important().to_string()
                title="Important"
                aria-label="Important"
                disabled=move || c.read_only()
                on:click=toggle_important
            >
                {icon(Icon::Star)}
            </button>
            <button class="icon-button close" aria-label="Close" title="Close (Esc)" aria-keyshortcuts="Escape"
                on:click=move |_| c.select(None)>
                {icon(Icon::X)}
            </button>
        </div>
    }
}

#[component]
fn DetailCheck(d: Detail) -> impl IntoView {
    let c = ctx();
    let id = d.id;
    let done = d.done;
    let check_row = c
        .shown
        .with_untracked(|rows| rows.iter().find(|s| s.row.id == id).map(|s| s.row.clone()));
    let on_check = move |_| {
        if done {
            c.uncomplete(id);
        } else if let Some(r) = check_row.clone() {
            c.complete(&r);
        } else {
            c.submit(Command::CompleteTask { id });
        }
    };
    view! {
        <button
            class="check big"
            role="checkbox"
            disabled=move || c.read_only()
            aria-checked=done.to_string()
            aria-label=if done { "Reopen" } else { "Complete" }
            on:click=on_check
        >
            {tick_svg()}
        </button>
    }
}

#[component]
fn DetailSteps(d: Detail) -> impl IntoView {
    let c = ctx();
    let steps_done = d.steps.iter().filter(|s| s.done).count();
    let steps_total = d.steps.len();
    view! {
        <h3 class="caps d-caps">
            "Steps"
            {(steps_total > 0)
                .then(|| view! { <span class="steps">{bars(steps_done, steps_total)}{format!("{steps_done}/{steps_total}")}</span> })}
        </h3>
        <ul class="step-rows">
            {d
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let sid = s.id;
                    let sdone = s.done;
                    view! {
                        <li class="step" class:done=sdone>
                            <button
                                class="check small"
                                role="checkbox"
                                disabled=move || c.read_only()
                                aria-checked=sdone.to_string()
                                aria-label=format!("Step {}", i + 1)
                                on:click=move |_| {
                                    let cmd = if sdone {
                                        Command::UncompleteStep { id: sid }
                                    } else {
                                        Command::CompleteStep { id: sid }
                                    };
                                    c.submit(cmd);
                                }
                            >
                                {tick_svg()}
                            </button>
                            <span class="step-title">{s.title.clone()}</span>
                            <span class="step-n">{format!("{:02}", i + 1)}</span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn DetailRest(d: Detail) -> impl IntoView {
    view! {
    <section class="d-section" aria-label="Read-only task fields">
    <h3 class="caps d-caps">"Task fields · Read only"</h3>
    <dl class="props">
        <div class="prop">
            <dt>{icon(Icon::Calendar)}"Due"</dt>
            <dd class="mono">{d.due.clone().unwrap_or_else(|| "—".to_owned())}</dd>
        </div>
        <div class="prop">
            <dt>{icon(Icon::Bell)}"Reminder"</dt>
            <dd class="mono">{d.reminder.clone().unwrap_or_else(|| "—".to_owned())}</dd>
        </div>
        <div class="prop">
            <dt>{icon(Icon::Repeat)}"Repeat"</dt>
            <dd>{d.repeat.clone()}</dd>
        </div>
        <div class="prop">
            <dt>{icon(Icon::Hash)}"Tags"</dt>
            <dd class="mono">
                {if d.tags.is_empty() {
                    "—".to_owned()
                } else {
                    d.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join("  ")
                }}
            </dd>
        </div>
    </dl>
    </section>

    {(!d.sources.is_empty())
        .then(|| {
            view! {
                <section class="d-section">
                    <h3 class="caps d-caps">"Source"</h3>
                    {d.sources.clone().into_iter().map(source_card).collect_view()}
                </section>
            }
        })}

    {(!d.attachments.is_empty())
        .then(|| {
            view! {
                <section class="d-section">
                    <h3 class="caps d-caps">"Attachments"</h3>
                    <ul class="files">
                        {d
                            .attachments
                            .clone()
                            .into_iter()
                            .map(|a| {
                                let (aid, name) = (a.id.clone(), a.name.clone());
                                view! {
                                    <li>
                                        <button
                                            class="file"
                                            on:click=move |_| {
                                                let (aid, name) = (aid.clone(), name.clone());
                                                leptos::task::spawn_local(platform::download(aid, name));
                                            }
                                        >
                                            {icon(Icon::Paperclip)}
                                            <span class="file-name">{a.name.clone()}</span>
                                            <span class="file-size">{a.size.clone()}</span>
                                            {icon(Icon::Download)}
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                </section>
            }
        })}

    <section class="d-section">
        <h3 class="caps d-caps">"Activity"</h3>
        <ul class="activity">
            {d
                .activity
                .iter()
                .map(|a| {
                    view! {
                        <li>
                            {match &a.mark {
                                Some(m) => mark(m).into_any(),
                                None => you_mark().into_any(),
                            }}
                            <span class="act-text"><b>{a.who.clone()}</b>" "{a.what.clone()}</span>
                            <span class="act-when">{a.when.clone()}</span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    </section>
    }
}

fn source_card(s: Source) -> impl IntoView {
    let (lead, title, meta, href) = match s {
        Source::Email {
            subject,
            meta,
            href,
        } => (Icon::Mail, subject, meta, href),
        Source::Web { title, href } => (Icon::Link, title, href.clone(), Some(href)),
    };
    let inner = move |arrow: bool| {
        view! {
            <span class="source-icon">{icon(lead)}</span>
            <span class="source-text">
                <span class="source-title">{title.clone()}</span>
                <span class="source-meta">{meta.clone()}</span>
            </span>
            {arrow.then(|| icon(Icon::ArrowUpRight))}
        }
    };
    match href {
        Some(h) => view! {
            <a class="source" href=h target="_blank" rel="noopener noreferrer">{inner(true)}</a>
        }
        .into_any(),
        None => view! { <div class="source">{inner(false)}</div> }.into_any(),
    }
}

/// An editor's draft over a field that live updates may change: while the draft is dirty (typed
/// in and not yet saved) it is kept; otherwise it follows the field.
#[derive(Clone, Copy)]
struct Draft {
    text: RwSignal<String>,
    dirty: RwSignal<bool>,
}

impl Draft {
    fn new(field: Signal<String>) -> Self {
        let d = Self {
            text: RwSignal::new(field.get_untracked()),
            dirty: RwSignal::new(false),
        };
        Effect::new(move |_| {
            let v = field.get();
            if !d.dirty.get_untracked() {
                d.text.set(v);
            }
        });
        d
    }

    fn input(self, v: String) {
        self.text.set(v);
        self.dirty.set(true);
    }

    /// Back to the field's value.
    fn reset(self, field: Signal<String>) {
        self.dirty.set(false);
        self.text.set(field.get_untracked());
    }
}

#[component]
fn TitleEditor(id: TaskId, title: Signal<String>) -> impl IntoView {
    let c = ctx();
    let input = NodeRef::<html::Input>::new();
    let draft = Draft::new(title);
    let commit = move || {
        if !c.editing.get_untracked() {
            return;
        }
        c.editing.set(false);
        let t = draft.text.get_untracked().trim().to_owned();
        let current = title.get_untracked();
        draft.dirty.set(false);
        if !t.is_empty() && t != current {
            c.submit(Command::UpdateTask {
                id,
                patch: TaskPatch {
                    title: Some(t),
                    ..TaskPatch::default()
                },
            });
        }
        draft.reset(title);
    };
    Effect::new(move |_| {
        if c.editing.get()
            && let Some(i) = input.get()
        {
            let _ = i.focus();
            i.select();
        }
    });
    view! {
        {move || {
            if c.editing.get() {
                view! {
                    <input
                        class="title-input"
                        node_ref=input
                        disabled=move || c.read_only()
                        aria-label="Title"
                        prop:value=move || draft.text.get()
                        on:input=move |ev| draft.input(event_target_value(&ev))
                        on:keydown=move |ev| {
                            match ev.key().as_str() {
                                "Enter" => {
                                    ev.prevent_default();
                                    commit();
                                    request_animation_frame(|| { platform::focus("edit-task-title"); });
                                }
                                "Escape" => {
                                    ev.prevent_default();
                                    ev.stop_propagation();
                                    draft.reset(title);
                                    c.editing.set(false);
                                    request_animation_frame(|| { platform::focus("edit-task-title"); });
                                }
                                _ => {}
                            }
                        }
                        on:blur=move |_| commit()
                    />
                }
                    .into_any()
            } else {
                view! {
                    <h2 class="d-title">
                        <button type="button" id="edit-task-title" disabled=move || c.read_only()
                            title="Edit title (E)" aria-keyshortcuts="E"
                            on:click=move |_| c.editing.set(true)>{move || title.get()}</button>
                    </h2>
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn NotesEditor(id: TaskId, notes: Signal<String>) -> impl IntoView {
    let c = ctx();
    let draft = Draft::new(notes);
    view! {
        <textarea
            class="notes"
            aria-label="Notes"
            readonly=move || c.read_only()
            placeholder="Add a note"
            rows="3"
            prop:value=move || draft.text.get()
            on:input=move |ev| draft.input(event_target_value(&ev))
            on:change=move |_| {
                let v = draft.text.get_untracked();
                draft.dirty.set(false);
                if v != notes.get_untracked() {
                    c.submit(Command::UpdateTask {
                        id,
                        patch: TaskPatch {
                            notes: Some(v),
                            ..TaskPatch::default()
                        },
                    });
                }
            }
        ></textarea>
    }
}

#[component]
fn AddStep(id: TaskId) -> impl IntoView {
    let c = ctx();
    let value = RwSignal::new(String::new());
    view! {
        <form
            class="add-step"
            on:submit=move |ev| {
                ev.prevent_default();
                let title = value.get_untracked().trim().to_owned();
                if title.is_empty() {
                    return;
                }
                let cmd = Command::CreateStep {
                    id: StepId::from_uuid(backend::new_uuid()),
                    task: id,
                    title,
                    placement: Placement::Last,
                };
                if c.submit(cmd) {
                    value.set(String::new());
                }
            }
        >
            {icon(Icon::Plus)}
            <input
                type="text"
                placeholder="Add step"
                aria-label="Add step"
                disabled=move || c.read_only()
                autocomplete="off"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
        </form>
    }
}

// ---- status bar ----------------------------------------------------------------------------------------------

#[component]
fn StatusBar() -> impl IntoView {
    let c = ctx();
    view! {
        <footer class="statusbar" inert=move || c.detail_modal.get()>
            <span class="sb-counts">
                {move || c.view_room.with(|r| format!("{} open   {} done", r.open(), r.done))}
            </span>
            <span class="sb-keys" aria-hidden="true">
                <Show when=move || !c.shown.with(Vec::is_empty)>
                    <span><kbd class="keycap">"J"</kbd><kbd class="keycap">"K"</kbd>" move"</span>
                </Show>
                <Show when=move || !c.read_only() && c.selected.with(|id| id.is_some_and(|id|
                    c.shown.with(|rows| rows.iter().any(|s| s.row.id == id && s.phase.is_none()))))>
                    <span><kbd class="keycap">"Space"</kbd>" toggle completion"</span>
                </Show>
                <Show when=move || !c.read_only() && c.detail.with(Option::is_some)>
                    <span><kbd class="keycap">"E"</kbd>" edit title"</span>
                </Show>
                <Show when=move || !c.detail_modal.get() && !c.read_only() && c.view_room.with(|r| r.add.is_some())>
                    <span><kbd class="keycap">"N"</kbd>" add task"</span>
                </Show>
                <Show when=move || c.detail.with(Option::is_some)>
                    <span><kbd class="keycap">"Esc"</kbd>" close detail"</span>
                </Show>
            </span>
        </footer>
    }
}
