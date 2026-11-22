//! The Leptos components. Layout and look are in `static/app.css`, which takes every value from
//! `tokens.css`; the view supplies state, including the user's preferred docked detail width.

mod activity;
mod attachments;
mod detail_splitter;
mod device_pairing;
mod links;
mod pickers;
mod reminder;
mod repeat;
mod room_actions;
mod tags;
mod task_actions;

use crate::backend::{self, Sync, UndoTarget};
use crate::changes::{self, Failure};
use crate::landscape::{self, Sky};
use crate::model::{
    self, AgentMark, Detail, DueTone, Now, Room, RoomId, Row, Sidebar, Source, Tomorrow,
};
use crate::platform;
use leptos::html;
use leptos::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use todo_client::room_header::Summary;
use todo_client::{CommandId, Connection, SubmitError, TrackedUndo, intent};
use todo_core::ids::{CollectionId, ListId, StepId, TaskId};
use todo_core::model::{Priority, Stamp};
use todo_core::{Command, State};
use todo_design::landscape::{HEIGHT_COVER, HEIGHT_DESKTOP, Landscape};
use todo_design::{Glyph, Icon, motion};

/// Where a completing row is in its choreography.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Fill, tick and hold: the row stays in place, checked and struck through.
    Hold,
    /// A completed row collapsing out of the list.
    Exit,
    /// A deleted or moved row collapsing out of this room.
    Depart,
}

impl Phase {
    fn exiting(phase: Option<Self>) -> bool {
        matches!(phase, Some(Self::Exit | Self::Depart))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowSection {
    Open,
    Completed,
    Handover,
}

/// A row that has left the data but is still shown where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linger {
    token: u64,
    room: RoomId,
    section: RowSection,
    row: Row,
    index: usize,
    completion_stamp: Option<Stamp>,
    phase: Phase,
}

/// A row as the list renders it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shown {
    row: Row,
    phase: Option<Phase>,
}

struct Undo {
    target: UndoTarget,
    label: String,
    animated_task: Option<TaskId>,
}

#[derive(Clone)]
struct WaitingUndo {
    toast_seq: u64,
    label: String,
}

/// The toast at the bottom of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    id: u64,
    text: String,
    undo: bool,
}

/// Shared UI state and whether it follows the real clock (`false` for fixtures).
#[derive(Clone, Copy)]
pub struct Ctx {
    state: RwSignal<Arc<State>>,
    sync: RwSignal<Sync>,
    sign_in_started: StoredValue<bool>,
    synced_at: RwSignal<Option<Now>>,
    now: RwSignal<Now>,
    live: bool,
    room: RwSignal<RoomId>,
    selected: RwSignal<Option<TaskId>>,
    lingering: RwSignal<Vec<Linger>>,
    toast: RwSignal<Option<Toast>>,
    toast_seq: RwSignal<u64>,
    undo: RwSignal<Option<Undo>>,
    waiting_undo: RwSignal<BTreeMap<CommandId, WaitingUndo>>,
    activity_undos: activity::Undos,
    activity_feed: RwSignal<activity::FeedState>,
    activity_revision: RwSignal<Option<(Option<todo_client::Epoch>, u64)>>,
    room_actions: room_actions::Controls,
    device_settings_open: RwSignal<bool>,
    task_actions: task_actions::Controls,
    failures: RwSignal<Vec<Failure>>,
    dark: RwSignal<bool>,
    drawer: RwSignal<bool>,
    narrow: RwSignal<bool>,
    detail_modal: Memo<bool>,
    collapsed: RwSignal<BTreeSet<CollectionId>>,
    new_room: RwSignal<Option<NewRoomKind>>,
    filter_open: RwSignal<bool>,
    filter_text: RwSignal<String>,
    completed_open: RwSignal<bool>,
    columns: Memo<Columns>,
    editing: RwSignal<bool>,
    quick_add: NodeRef<html::Input>,
    view_room: Memo<Room>,
    finished: Memo<bool>,
    tomorrow: Memo<Option<Tomorrow>>,
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
        let state = backend::view();
        let s = backend::status();
        if s.connection == Connection::Online && s.pending == 0 {
            self.collapsed.update(|ids| {
                ids.retain(|id| {
                    let exists = state.is_live((*id).into());
                    if !exists {
                        platform::save_folded(*id, false);
                    }
                    exists
                });
            });
        }
        self.state.set(state);
        if s.connection == Connection::Online && !s.offline_demo {
            self.synced_at.set(Some(platform::now()));
        }
        if s.offline_demo && self.synced_at.get_untracked().is_none() {
            self.synced_at.set(Some(self.now.get_untracked()));
        }
        // The revocation latch also covers Activity and attachments. Only this path navigates:
        // a second location change could cancel the first before the login document commits.
        // Wait for local writes first, so the beforeunload guard cannot cancel our one attempt.
        if s.signed_out && !self.sign_in_started.get_value() && !backend::unsaved() {
            self.sign_in_started.set_value(true);
            platform::sign_in(&platform::path());
        }
        self.sync.set(s);
        self.failures.set(backend::failures());
        let revision = backend::activity_revision();
        if self.activity_revision.get_untracked() != revision {
            self.activity_revision.set(revision);
        }
        self.activity_undos.settle();
        for (id, undo) in self.waiting_undo.get_untracked() {
            if let Some(outcome) = backend::undo_result(id) {
                self.waiting_undo.update(|pending| {
                    pending.remove(&id);
                });
                // A late answer must not take the Undo button away from a newer change.
                if undo.toast_seq == self.toast_seq.get_untracked() {
                    let label = match outcome {
                        Ok(()) => format!("Undid: {}", undo.label),
                        Err(message) => format!("{message} ({})", undo.label),
                    };
                    self.show_toast(label, false);
                }
            }
        }
    }

    /// A newer version took the database over: no more edits here (see `backend::submit`).
    fn read_only(self) -> bool {
        self.sync.with(|s| s.read_only)
    }

    fn submit(self, cmd: Command) -> bool {
        self.submission(backend::submit(cmd)).is_some()
    }

    fn submission<T>(self, result: Result<T, SubmitError>) -> Option<T> {
        match result {
            Ok(value) => {
                self.refresh();
                Some(value)
            }
            // Nothing was applied; the persistent alert says why and offers a reload.
            Err(_) if backend::status().read_only => {
                self.refresh();
                None
            }
            Err(error) => {
                tracing::debug!(%error, "could not queue change");
                self.show_toast(changes::submission_error(&error), false);
                None
            }
        }
    }

    fn submit_intent(self, build: impl FnOnce(&State) -> Option<Command>) -> bool {
        let command = self.state.with_untracked(|state| build(state));
        command.is_some_and(|command| self.submit(command))
    }

    fn submit_undoable(self, cmd: Command, label: String, animated_task: Option<TaskId>) -> bool {
        let Some(target) = self.submission(backend::submit_tracked(cmd)) else {
            return false;
        };
        self.undo.set(Some(Undo {
            target,
            animated_task,
            label: label.clone(),
        }));
        self.show_toast(label, true);
        true
    }

    fn undo(self) {
        let Some(action) = self.undo.try_update(Option::take).flatten() else {
            return;
        };
        self.toast.set(None);
        let result = backend::undo(&action.target);
        if matches!(result, Ok(TrackedUndo::Cancelled | TrackedUndo::Queued(_)))
            && let Some(id) = action.animated_task
        {
            self.lingering.update(|l| l.retain(|x| x.row.id != id));
        }
        match result {
            Ok(TrackedUndo::Cancelled) => self.show_toast("Change undone.".into(), false),
            Ok(TrackedUndo::Queued(id)) => {
                let toast_seq = self.toast_seq.get_untracked();
                self.waiting_undo.update(|pending| {
                    pending.insert(
                        id,
                        WaitingUndo {
                            toast_seq,
                            label: action.label,
                        },
                    );
                });
            }
            Ok(TrackedUndo::Unavailable) => {
                self.show_toast(changes::undo_failure("undo_no_event").into(), false)
            }
            Err(message) => self.show_toast(message, false),
        }
        self.refresh();
    }

    fn fold(self, id: CollectionId, folded: bool) {
        self.collapsed.update(|ids| {
            if folded {
                ids.insert(id);
            } else {
                ids.remove(&id);
            }
        });
        platform::save_folded(id, folded);
    }

    fn toggle_fold(self, id: CollectionId) {
        self.fold(id, !self.collapsed.with_untracked(|ids| ids.contains(&id)));
    }

    fn show_toast(self, text: String, undo: bool) {
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
        self.room_actions.close_menu(false);
        let _ = self.task_actions.close();
        self.lingering
            .update(|rows| rows.retain(|l| l.phase != Phase::Depart));
        self.drawer.set(false);
        self.new_room.set(None);
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
        let handover = self.tomorrow.with_untracked(|plan| {
            plan.as_ref()
                .is_some_and(|plan| plan.rows().any(|r| r.id == row.id))
        });
        let id = row.id;
        let Some((cmd, updated)) = self.state.with_untracked(|s| {
            Some((
                intent::set_done(s, id, true)?,
                s.tasks.get(&id)?.updated.clone(),
            ))
        }) else {
            return;
        };
        if !self.submit_undoable(cmd, format!("Completed “{}”", row.title), Some(id)) {
            return;
        }
        // A rapid Undo followed by another completion starts a fresh timer for the same task.
        let token = self.toast_seq.get_untracked();
        self.lingering.update(|l| {
            l.retain(|x| x.row.id != id);
            l.push(Linger {
                token,
                room,
                section: if handover {
                    RowSection::Handover
                } else {
                    RowSection::Open
                },
                row: row.clone(),
                index: 0, // The room model keeps held completions in order.
                completion_stamp: Some(updated),
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
                    for x in l.iter_mut().filter(|x| x.token == token) {
                        x.phase = Phase::Exit;
                    }
                });
                set_timeout(
                    move || self.lingering.update(|l| l.retain(|x| x.token != token)),
                    Duration::from_millis(u64::from(motion::ROW_COLLAPSE)),
                );
            },
            Duration::from_millis(u64::from(hold)),
        );
    }

    fn uncomplete(self, id: TaskId) {
        self.lingering.update(|l| l.retain(|x| x.row.id != id));
        self.submit_intent(|state| intent::set_done(state, id, false));
    }

    fn toggle(self, row: &Row) {
        if row.done {
            self.uncomplete(row.id);
        } else {
            self.complete(row);
        }
    }

    fn actionable_rows(self) -> Vec<Row> {
        if let Some(plan) = self.tomorrow.get_untracked() {
            return self.lingering.with_untracked(|leaving| {
                plan.rows()
                    .filter(|r| !r.done && handover_row_phase(leaving, r.id).is_none())
                    .cloned()
                    .collect()
            });
        }
        self.shown.with_untracked(|rows| {
            rows.iter()
                .filter(|s| s.phase.is_none())
                .map(|s| s.row.clone())
                .collect()
        })
    }

    /// Moves the selection by `delta` rows (J/K), including the finished day's hand-over.
    fn step_selection(self, delta: isize) {
        let rows = self.actionable_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self
            .selected
            .get_untracked()
            .and_then(|id| rows.iter().position(|r| r.id == id));
        let next = match cur {
            None => 0,
            Some(i) => i
                .saturating_add_signed(delta)
                .min(rows.len().saturating_sub(1)),
        };
        if let Some(row) = rows.get(next) {
            self.select(Some(row.id));
            if !self.detail_modal.get_untracked() {
                platform::focus(&format!("row-{}", row.id));
            }
        }
    }
}

/// Starts the app: reads the backend into signals and mounts the page. `notice` is shown once, as a
/// toast (what the start had to repair).
pub fn mount(now: Now, live: bool, notice: Option<String>) {
    leptos::mount::mount_to_body(move || view! { <App now=now live=live notice=notice.clone()/> });
}

fn current_route() -> (RoomId, Option<TaskId>) {
    let path = platform::path();
    let (room, task) = RoomId::from_path(&path);
    if room.is_activity() {
        let canonical = room.path();
        if canonical != path {
            platform::replace_path(&canonical);
        }
    }
    (room, task)
}

#[component]
fn App(now: Now, live: bool, notice: Option<String>) -> impl IntoView {
    let (room0, task0) = current_route();
    let state = RwSignal::new(backend::view());
    let now = RwSignal::new(now);
    let room = RwSignal::new(room0);
    let lingering = RwSignal::new(Vec::<Linger>::new());
    let filter_text = RwSignal::new(String::new());
    // Only completions count as open until they have left; moves/deletes are paint only.
    let holding = Memo::new(move |_| {
        let id = room.get();
        lingering.with(|l| {
            l.iter()
                .filter(|x| x.section == RowSection::Open && x.room == id)
                .filter_map(|x| Some((x.row.id, x.completion_stamp.clone()?)))
                .collect::<BTreeMap<_, _>>()
        })
    });
    let (_, view_room) = model::room_memos(state, now, room, holding, filter_text);
    let shown = Memo::new(move |_| {
        view_room.with(|r| lingering.with(|l| rows_with_exits(r, RowSection::Open, l)))
    });
    let finished = Memo::new(move |_| {
        view_room.with(Room::finished)
            && lingering.with(|l| {
                l.iter()
                    .all(|x| x.section != RowSection::Open || x.room != RoomId::default())
            })
    });
    let tomorrow = Memo::new(move |_| {
        (finished.get() && !view_room.with(|r| r.filtered)).then(|| {
            lingering.with(|leaving| {
                state.with(|s| {
                    model::tomorrow(
                        s,
                        &now.get(),
                        |id| {
                            leaving.iter().any(|x| {
                                x.section == RowSection::Handover
                                    && x.row.id == id
                                    && x.phase != Phase::Depart
                            })
                        },
                        |id| {
                            leaving.iter().any(|x| {
                                x.section == RowSection::Handover
                                    && x.row.id == id
                                    && x.phase == Phase::Depart
                            })
                        },
                    )
                })
            })
        })
    });
    let selected = RwSignal::new(task0);
    let detail = Memo::new(move |_| {
        selected
            .get()
            .and_then(|id| state.with(|s| model::detail(s, &now.get(), id)))
    });
    let sidebar = Memo::new(move |_| {
        let now = now.get();
        state.with(|s| model::sidebar(s, &now, |at| platform::civil_time(at, now, live).date()))
    });
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
        sign_in_started: StoredValue::new(false),
        synced_at: RwSignal::new(None),
        now,
        live,
        room,
        selected,
        lingering,
        toast: RwSignal::new(None),
        toast_seq: RwSignal::new(0),
        undo: RwSignal::new(None),
        waiting_undo: RwSignal::new(BTreeMap::new()),
        activity_undos: activity::Undos::new(),
        activity_feed: RwSignal::new(activity::FeedState::default()),
        activity_revision: RwSignal::new(None),
        room_actions: room_actions::Controls::new(),
        device_settings_open: RwSignal::new(false),
        task_actions: task_actions::Controls::new(),
        failures: RwSignal::new(Vec::new()),
        dark: RwSignal::new(platform::prefers_dark()),
        drawer: RwSignal::new(false),
        narrow,
        detail_modal,
        collapsed: RwSignal::new(platform::load_folded().unwrap_or_default()),
        new_room: RwSignal::new(None),
        filter_open: RwSignal::new(false),
        filter_text,
        completed_open,
        columns,
        editing: RwSignal::new(false),
        quick_add: NodeRef::new(),
        view_room,
        finished,
        tomorrow,
        shown,
        detail,
        sidebar,
    };
    provide_context(c);
    c.refresh();
    if let Some(n) = notice {
        c.show_toast(n, false);
    }

    // Theme and room hue live on <html>, where tokens.css looks for them.
    platform::on_media_change("(prefers-color-scheme: dark)", move |dark| c.dark.set(dark));
    platform::on_media_change(&platform::detail_overlay_query(), move |n| c.narrow.set(n));
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
        let (r, t) = current_route();
        if r != c.room.get_untracked() {
            c.reset_room_controls();
        }
        c.room.set(r);
        c.selected.set(t);
        c.editing.set(false);
    });

    let _keys = window_event_listener(leptos::ev::keydown, move |ev| on_key(c, &ev));
    let _folds = window_event_listener(leptos::ev::storage, move |ev| {
        if platform::is_fold_key(ev.key().as_deref())
            && let Some(ids) = platform::load_folded()
        {
            c.collapsed.set(ids);
        }
    });

    if live {
        // Follow the sync engine, and keep "today" current.
        leptos::task::spawn_local(async move {
            loop {
                backend::next_change().await;
                c.refresh();
            }
        });
        platform::on_wake(move || {
            c.now.set(platform::now());
            backend::nudge();
        });
    }
    if now.with_untracked(|n| n.browser_time) {
        set_interval(move || c.now.set(platform::now()), Duration::from_secs(30));
    }

    track_overlay_focus(c);
    let detail_layout = detail_splitter::Layout::new();
    let docked = Memo::new(move |_| !narrow.get() && detail.with(Option::is_some));
    let app = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        let width = detail_layout.width.get();
        let overridden = docked.get() && width != detail_splitter::DEFAULT;
        if let Some(app) = app.get() {
            let style = web_sys::HtmlElement::style(&app);
            if overridden {
                // Set one numeric CSSOM property, without inline style markup (the app's CSP).
                let _ = style.set_property("--detail-pane-width", &format!("{width}px"));
            } else {
                let _ = style.remove_property("--detail-pane-width");
                if style.length() == 0 {
                    let _ = app.remove_attribute("style");
                }
            }
        }
    });
    view! {
        <div class="app" node_ref=app class:has-detail=move || c.detail.with(Option::is_some)
            class:compact-room=move || docked.get() && detail_layout.compact_room.get()
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
            <detail_splitter::DetailSplitter layout=detail_layout docked=docked/>
            <DetailPane/>
            <StatusBar/>
        </div>
        <room_actions::Overlays/>
        <task_actions::Overlays/>
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
            let before = pickers::focused();
            request_animation_frame(move || {
                if c.selected.get_untracked().is_none()
                    && pickers::focus_unmoved(before.as_ref())
                    && !platform::focus(&format!("row-{old}"))
                {
                    platform::focus("room-title");
                }
            });
        }
        (modal, id, room)
    });
    Effect::new(move |prev: Option<(bool, RoomId)>| {
        let open = c.drawer.get();
        let room = c.room.get();
        if !open
            && prev
                .as_ref()
                .is_some_and(|(_, old)| old != &room && (old.is_activity() || room.is_activity()))
        {
            request_animation_frame(|| {
                platform::focus("room-title");
            });
        } else if open || prev.is_some_and(|(was_open, _)| was_open) {
            request_animation_frame(move || {
                if open == c.drawer.get_untracked() {
                    platform::focus(if open { "list-drawer" } else { "open-drawer" });
                }
            });
        }
        (open, room)
    });
}

fn typing(ev: &leptos::ev::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| {
            matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || el
                    .closest("[contenteditable]:not([contenteditable=false])")
                    .ok()
                    .flatten()
                    .is_some()
        })
}

fn on_key(c: Ctx, ev: &leptos::ev::KeyboardEvent) {
    use wasm_bindgen::JsCast;
    if ev.is_composing()
        || ev.default_prevented()
        || c.room_actions.dialog_open()
        || c.device_settings_open.get_untracked()
        || c.task_actions.open()
    {
        return;
    }
    let key = ev.key().to_ascii_lowercase();
    let drawer = c.drawer.get_untracked();
    let modal = c.detail_modal.get_untracked();
    if key == "tab" && (drawer || modal) {
        platform::contain_tab(if drawer { "list-drawer" } else { "task-detail" }, ev);
        return;
    }
    let typing = typing(ev);
    if ev.repeat() {
        // Leave typing alone, but prevent a held Enter from activating a button or submitting a form.
        let single_line = ev
            .target()
            .is_some_and(|t| t.is_instance_of::<web_sys::HtmlInputElement>());
        if (!typing
            && matches!(
                key.as_str(),
                "enter" | "escape" | " " | "arrowup" | "arrowdown"
            ))
            || (single_line && key == "enter")
        {
            ev.prevent_default();
        }
        return;
    }
    if key == "escape" {
        if c.editing.get_untracked() {
            c.editing.set(false);
        } else if !modal && c.new_room.get_untracked().is_some() {
            c.new_room.set(None);
            platform::focus("open-new-room");
        } else if drawer {
            c.drawer.set(false);
        } else if c.selected.get_untracked().is_some() {
            c.select(None);
        } else if c.filter_open.get_untracked() {
            c.close_filter();
        }
        return;
    }
    let activity = c.room.with_untracked(RoomId::is_activity);
    if !typing
        && !activity
        && !drawer
        && !modal
        && !ev.alt_key()
        && !ev.meta_key()
        && ((ev.ctrl_key() && key == "k") || (!ev.ctrl_key() && key == "/"))
    {
        ev.prevent_default();
        c.open_filter();
        return;
    }
    if typing || ev.meta_key() {
        return;
    }
    if ev.ctrl_key() {
        if key == "z"
            && !ev.shift_key()
            && !ev.alt_key()
            && !c.read_only()
            && c.undo.with_untracked(Option::is_some)
        {
            ev.prevent_default();
            c.undo();
        }
        return;
    }
    if drawer || activity {
        // The history has no task selection or composer. Keep native scrolling and button keys.
        return;
    }
    if ev.alt_key() {
        if !ev.shift_key()
            && !c.read_only()
            && matches!(key.as_str(), "arrowup" | "arrowdown")
            && let (Some(id), room @ RoomId::List(_)) =
                (c.selected.get_untracked(), c.room.get_untracked())
        {
            ev.prevent_default();
            let by = if key == "arrowup" { -1 } else { 1 };
            if let Some(cmd) = c
                .state
                .with_untracked(|s| model::reorder_task(s, &room, id, by))
            {
                let positions = platform::row_positions();
                if c.submit_undoable(cmd, "Task moved.".into(), None) {
                    request_animation_frame(move || {
                        if c.room.get_untracked() == room {
                            platform::animate_reorder(positions);
                            if !modal {
                                platform::refocus_row(&format!("row-{id}"));
                            }
                        }
                    });
                }
            }
        }
        return;
    }
    if !c.read_only() && task_actions::shortcut(c, ev) {
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
            // Space may complete from the selected row, but not from other focused controls.
            if ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|el| {
                    el.get_attribute("role").as_deref() == Some("separator")
                        || (matches!(el.tag_name().as_str(), "BUTTON" | "A")
                            && !selected.is_some_and(|id| el.id() == format!("row-{id}")))
                })
            {
                return;
            }
            if let Some(row) = c
                .actionable_rows()
                .into_iter()
                .find(|r| Some(r.id) == selected)
            {
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
        "r" | "t" | "l"
            if !ev.shift_key() && !c.read_only() && c.detail.with_untracked(Option::is_some) =>
        {
            ev.prevent_default();
            match key.as_str() {
                "l" => {
                    platform::focus("add-task-link");
                }
                _ => {
                    let id = if key == "r" {
                        "edit-task-repeat"
                    } else {
                        "edit-task-tags"
                    };
                    if platform::focus(id)
                        && let Some(button) = document()
                            .get_element_by_id(id)
                            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        button.click();
                    }
                }
            }
        }
        "d" | "p" if !c.read_only() => {
            let Some(id) = c.detail.with_untracked(|d| d.as_ref().map(|d| d.id)) else {
                return;
            };
            ev.prevent_default();
            if key == "d" {
                if let Some(button) = document()
                    .get_element_by_id("edit-task-due")
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    button.click();
                }
            } else if ev.shift_key() {
                platform::focus_select("task-priority");
            } else if c.submit_intent(|s| intent::cycle_priority(s, id)) && !modal {
                request_animation_frame(move || {
                    platform::refocus_row(&format!("row-{id}"));
                });
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

/// Render both shapes so a state change can cross-fade without remounting the button.
fn toggle_icon(outline: Icon, filled: Icon) -> impl IntoView {
    view! {
        <span class="toggle-icon" aria-hidden="true">
            <span class="icon toggle-outline" inner_html=outline.svg()></span>
            <span class="icon toggle-fill" inner_html=filled.svg()></span>
        </span>
    }
}

fn glyph(g: Glyph) -> impl IntoView {
    view! { <span class="glyph" data-glyph=g.key() aria-hidden="true" inner_html=g.svg()></span> }
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

fn sync_text(s: &Sync, at: Option<Now>) -> todo_client::sync_label::SyncText {
    use todo_client::sync_label::SyncLabel;

    let time = at.map(|n| {
        let l = n.local();
        format!("{:02}:{:02}", l.hour(), l.minute())
    });
    let label = if s.signed_out {
        SyncLabel::SignedOut
    } else if s.failed > 0 {
        SyncLabel::Failed(s.failed)
    } else {
        match s.connection {
            Connection::Offline => SyncLabel::Offline(s.pending),
            Connection::Syncing => SyncLabel::Syncing,
            Connection::Online => SyncLabel::Synced(time.as_deref()),
        }
    };
    label.text()
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
    let item = move |n: model::NavItem, nested: bool, children: Vec<RoomId>, pip: usize| {
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
        let origin = room_actions::Subject::of(&room)
            .map(|s| room_actions::Origin(s, room_actions::Anchor::Link));
        let collection = match room {
            RoomId::Collection(id) => Some(id),
            _ => None,
        };
        let disclosure = collection.map(|id| {
            let label = n.name.clone();
            let folded = move || c.collapsed.with(|ids| ids.contains(&id));
            view! {
                <button type="button" class="collection-toggle"
                    aria-expanded=move || (!folded()).to_string()
                    aria-controls=format!("collection-{id}")
                    aria-label=move || format!("{} {label}", if folded() { "Expand" } else { "Collapse" })
                    on:click=move |_| c.toggle_fold(id)
                >{icon(Icon::ChevronRight)}</button>
            }
        });
        let (lead, count) = match (&n.room, n.glyph) {
            (RoomId::Smart(id), _) => (
                icon(model::smart_icon(id)).into_any(),
                (*id != "agent_activity").then_some(n.count),
            ),
            (RoomId::Activity(_), _) => (icon(Icon::Clock).into_any(), None),
            (_, Some(g)) => (glyph(g).into_any(), Some(n.count)),
            _ => (().into_any(), None),
        };
        let link = view! {
            <a
                id=origin.map(room_actions::Origin::dom_id)
                class="nav-item"
                class:menu-anchor=move || origin.is_some_and(|o| c.room_actions.anchored(o))
                class:sub=nested
                class:active=active.clone()
                class:contains-active=contains_active
                aria-current=move || active().then_some("page")
                href=href
                data-list-hue=n.hue
                on:click=link_room(c, room)
                on:contextmenu=move |ev| {
                    if let Some(origin) = origin.filter(|_| !c.read_only()) {
                        ev.prevent_default();
                        c.room_actions.open_menu(c, origin);
                    }
                }
                on:keydown=move |ev| {
                    if let Some(origin) = origin.filter(|_| room_actions::context_key(&ev) && !c.read_only()) {
                        ev.prevent_default();
                        if !ev.repeat() { c.room_actions.open_menu(c, origin); }
                    }
                }
            >
                {lead}
                <span class="nav-name">{n.name.clone()}</span>
                {count.map(|n| view! { <span class="count">{n.to_string()}</span> })}
                {(pip > 0).then(|| view! {
                    <span class="count nav-pip"><i aria-hidden="true"></i>{format!("{pip} new")}</span>
                })}
            </a>
        };
        if let Some(room_actions::Origin(subject, _)) = origin {
            view! {
                <div class="nav-entry" class:nav-collection=collection.is_some()>
                    {disclosure}
                    {link}
                    <room_actions::MenuButton origin=room_actions::Origin(subject, room_actions::Anchor::Sidebar)
                        label=format!("Menu for {}", n.name)/>
                </div>
            }.into_any()
        } else {
            link.into_any()
        }
    };
    let navigation = view! {
            <nav class="nav" on:scroll=move |_| c.room_actions.position_menu()>
                {move || {
                    let sb = c.sidebar.get();
                    view! {
                        <div class="nav-section">
                            {sb.smart.into_iter().map(|n| {
                                let pip = if n.room == RoomId::Smart("agent_activity") { sb.agent_updates_today } else { 0 };
                                item(n, false, Vec::new(), pip)
                            }).collect_view()}
                            {item(model::NavItem {
                                room: RoomId::Activity(None), name: "Activity".into(), count: 0,
                                hue: "agent-activity", glyph: None,
                            }, false, Vec::new(), 0)}
                            {(!sb.agents.is_empty()).then(|| view! {
                                <section class="side-agents" aria-label="Agents">
                                    <ul>
                                        {sb.agents.into_iter().filter_map(|(agent, status)| {
                                            let room = RoomId::Activity(Some(todo_core::actor::AgentId::new(&agent.name).ok()?));
                                            let active_room = room.clone();
                                            let active = move || c.room.with(|r| *r == active_room);
                                            Some(view! {
                                                <li><a class="nav-item nav-agent" class:active=active.clone()
                                                    aria-current=move || active().then_some("page")
                                                    href=room.path() on:click=link_room(c, room)>
                                                    {mark(&agent)}<span class="nav-name">{model::agent_display(&agent.name)}</span>
                                                    <span class="count">{status}</span>
                                                </a></li>
                                            })
                                        }).collect_view()}
                                    </ul>
                                </section>
                            })}
                        </div>
                        {(!sb.groups.is_empty()).then(|| view! {
                            <div class="nav-collections">
                                {sb.groups.into_iter().filter_map(|g| {
                                    let RoomId::Collection(id) = g.head.room else { return None };
                                    let children = g.lists.iter().map(|n| n.room.clone()).collect();
                                    let folded = move || c.collapsed.with(|ids| ids.contains(&id));
                                    Some(view! {
                                        <div class="nav-section nav-group" class:folded=folded>
                                            {item(g.head, false, children, 0)}
                                            <div id=format!("collection-{id}") hidden=folded>
                                                {g.lists.into_iter().map(|n| item(n, true, Vec::new(), 0)).collect_view()}
                                            </div>
                                        </div>
                                    })
                                }).collect_view()}
                            </div>
                        })}
                        {(!sb.loose.is_empty()).then(|| view! {
                            <div class="nav-section">
                                {sb.loose.into_iter().map(|n| item(n, false, Vec::new(), 0)).collect_view()}
                            </div>
                        })}
                    }
                }}
            </nav>
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
            <div class="side-body" on:scroll=move |_| c.room_actions.position_menu()>
                {navigation}
                {move || c.new_room.get().map(|kind| view! { <NewRoom kind/> })}
            </div>
            <div class="side-foot">
                <NewRoomMenu/>
                <div class="side-sync">
                    <span class="conn" aria-hidden="true" data-conn=move || conn_key(&c.sync.get())></span>
                    <room_actions::SyncLine/>
                </div>
                <SettingsMenu/>
                <device_pairing::DeviceSettings/>
            </div>
        </aside>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NewRoomKind {
    List,
    Collection,
}

#[derive(Clone, Copy)]
struct FooterMenu {
    open: RwSignal<bool>,
    trigger: &'static str,
    items: &'static [&'static str],
}

impl FooterMenu {
    fn new(trigger: &'static str, items: &'static [&'static str]) -> Self {
        Self {
            open: RwSignal::new(false),
            trigger,
            items,
        }
    }

    fn toggle(self) {
        if self.open.get_untracked() {
            self.close();
        } else {
            self.open_at(0);
        }
    }

    fn open_at(self, index: usize) {
        self.open.set(true);
        request_animation_frame(move || {
            if self.open.get_untracked() {
                platform::focus(self.items[index]);
            }
        });
    }

    fn close(self) {
        self.open.set(false);
        platform::focus(self.trigger);
    }

    fn focus_out(self, ev: leptos::ev::FocusEvent) {
        use wasm_bindgen::JsCast;
        let inside = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .zip(
                ev.related_target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok()),
            )
            .is_some_and(|(menu, next)| menu.contains(Some(&next)));
        if !inside {
            self.open.set(false);
        }
    }

    fn key(self, ev: leptos::ev::KeyboardEvent) {
        use wasm_bindgen::JsCast;
        if ev.alt_key() || ev.ctrl_key() || ev.meta_key() || ev.is_composing() {
            return;
        }
        let last = self.items.len() - 1;
        let open = self.open.get_untracked();
        let from = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| self.items.iter().position(|id| *id == el.id()));
        let next = match ev.key().as_str() {
            "ArrowDown" => from.map_or(0, |i| (i + 1) % self.items.len()),
            "ArrowUp" => from.map_or(last, |i| i.checked_sub(1).unwrap_or(last)),
            "Home" if open => 0,
            "End" if open => last,
            "Tab" if open => {
                // Let normal Tab order (including the phone drawer's trap) start at the trigger.
                self.close();
                return;
            }
            "Escape" if open => {
                ev.prevent_default();
                ev.stop_propagation();
                self.close();
                return;
            }
            _ => return,
        };
        ev.prevent_default();
        ev.stop_propagation();
        if open {
            platform::focus(self.items[next]);
        } else {
            self.open_at(next);
        }
    }
}

#[component]
fn NewRoomMenu() -> impl IntoView {
    let c = ctx();
    let menu = FooterMenu::new(
        "open-new-room",
        &["create-list-choice", "create-collection-choice"],
    );
    Effect::new(move |_| {
        if c.read_only() {
            menu.open.set(false);
        }
    });
    let choose = move |kind| {
        if c.read_only() {
            return;
        }
        menu.close();
        c.new_room.set(Some(kind));
        request_animation_frame(move || {
            if c.new_room.get_untracked() == Some(kind) {
                platform::focus(kind.field());
            }
        });
    };
    view! {
        <div class="footer-menu" on:focusout=move |ev| menu.focus_out(ev) on:keydown=move |ev| menu.key(ev)>
            <button type="button" id=menu.trigger class="button footer-new" aria-haspopup="menu"
                aria-controls="new-room-menu" aria-expanded=move || menu.open.get().to_string()
                disabled=move || c.read_only() on:click=move |_| menu.toggle()>{icon(Icon::Plus)}"New"</button>
            <Show when=move || menu.open.get() && !c.read_only()>
                <div class="footer-popup new-room-menu" id="new-room-menu" role="menu" aria-labelledby=menu.trigger>
                    <button type="button" id=menu.items[0] role="menuitem" tabindex="-1"
                        on:click=move |_| choose(NewRoomKind::List)>{icon(Icon::Plus)}"List"</button>
                    <button type="button" id=menu.items[1] role="menuitem" tabindex="-1"
                        on:click=move |_| choose(NewRoomKind::Collection)>{icon(Icon::FolderPlus)}"Collection"</button>
                </div>
            </Show>
        </div>
    }
}

impl NewRoomKind {
    fn field(self) -> &'static str {
        match self {
            Self::List => "new-list-name",
            Self::Collection => "new-collection-name",
        }
    }
}

#[component]
fn NewRoom(kind: NewRoomKind) -> impl IntoView {
    let c = ctx();
    let (name, create, cancel) = match kind {
        NewRoomKind::List => ("List name", "Create list", "Cancel new list"),
        NewRoomKind::Collection => (
            "Collection name",
            "Create collection",
            "Cancel new collection",
        ),
    };
    let close = move || {
        c.new_room.set(None);
        platform::focus("open-new-room");
    };
    let input = NodeRef::<html::Input>::new();
    view! {
        <div class="new-list">
            <form on:submit=move |ev| {
                ev.prevent_default();
                if c.read_only() { return; }
                let Some(i) = input.get_untracked() else { return };
                let name = i.value();
                if name.trim().is_empty() { return; }
                let (command, destination, parent) = match kind {
                    NewRoomKind::List => {
                        let room = c.room.get_untracked();
                        let parent = c.state.with_untracked(|state| intent::list_collection(state, (&room).into()));
                        let id = ListId::from_uuid(backend::new_uuid());
                        (intent::create_list(id, parent, &name), RoomId::List(id), parent)
                    }
                    NewRoomKind::Collection => {
                        let id = CollectionId::from_uuid(backend::new_uuid());
                        (intent::organize::create_collection(id, &name), RoomId::Collection(id), None)
                    }
                };
                if c.submit(command) {
                    if let Some(group) = parent { c.fold(group, false); }
                    c.go(destination);
                    request_animation_frame(|| { platform::focus("room-title"); });
                }
            }>
                <input id=kind.field() node_ref=input type="text" autocomplete="off" required
                    aria-label=name placeholder=format!("{name}…") disabled=move || c.read_only()/>
                <button class="icon-button" type="submit" aria-label=create title=create
                    disabled=move || c.read_only()>{icon(Icon::Plus)}</button>
                <button class="icon-button" type="button" aria-label=cancel title="Cancel"
                    on:click=move |_| close()>{icon(Icon::X)}</button>
            </form>
        </div>
    }
}

#[component]
fn SettingsMenu() -> impl IntoView {
    let menu = FooterMenu::new("open-settings", &["sign-out"]);
    let csrf = backend::csrf();
    let field = NodeRef::<html::Input>::new();
    csrf.map(|token| {
        view! {
            <div class="footer-menu" on:focusout=move |ev| menu.focus_out(ev) on:keydown=move |ev| menu.key(ev)>
                <button type="button" id=menu.trigger class="icon-button" title="Settings" aria-label="Settings"
                    aria-haspopup="menu" aria-controls="settings-menu" aria-expanded=move || menu.open.get().to_string()
                    on:click=move |_| menu.toggle()>{icon(Icon::Settings)}</button>
                <Show when=move || menu.open.get()>
            <form
                class="footer-popup settings-menu" id="settings-menu" role="menu" aria-labelledby=menu.trigger
                method="post"
                action="/auth/logout"
                on:submit=move |_| {
                    // The token may have changed since the page loaded (see `backend::csrf`).
                    if let (Some(i), Some(t)) = (field.get(), backend::csrf()) {
                        i.set_value(&t);
                    }
                }
            >
                <input type="hidden" name="csrf" value=token.clone() node_ref=field/>
                <button type="submit" id=menu.items[0] role="menuitem" tabindex="-1">{icon(Icon::LogOut)}"Sign out"</button>
            </form>
                </Show>
            </div>
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
                        .then(|| sync_text(&s, c.synced_at.get()).full)
                }}
            </span>
            <Show when=move || !c.room.with(RoomId::is_activity)>
                <button type="button" class="filter-trigger" id="open-filter" aria-label="Filter tasks"
                    aria-keyshortcuts="/ Control+K" title="Filter tasks (/ or Ctrl+K)"
                    aria-expanded=move || c.filter_open.get().to_string()
                    on:click=move |_| c.open_filter()
                >{icon(Icon::Search)}<span>"Filter"</span><kbd class="keycap">"Ctrl K"</kbd></button>
            </Show>
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
                                if !ev.repeat() { c.close_filter(); }
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
                        let active = move || c.room.get() == active_room;
                        let lead = match (&n.room, n.glyph) {
                            (RoomId::Smart(id), _) => icon(model::smart_icon(id)).into_any(),
                            (_, Some(g)) => glyph(g).into_any(),
                            _ => ().into_any(),
                        };
                        view! {
                            <a
                                class="pill"
                                class:active=active.clone()
                                aria-current=move || active().then_some("page")
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
    let reached_bottom = RwSignal::new(0u64);
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
        <div class="room" node_ref=node on:scroll=move |_| {
            c.room_actions.position_menu();
            c.task_actions.position();
            if c.room.with_untracked(RoomId::is_activity) && let Some(el) = node.get_untracked() {
                let remaining = el.scroll_height() - el.scroll_top() - el.client_height();
                // Scroll metrics are rounded; also start before the last row leaves the viewport.
                if remaining <= el.client_height() / 4 {
                    reached_bottom.update(|n| *n += 1);
                }
            }
        }>
            <Show when=move || finished.get() fallback=move || view! { <RoomHeader width=width/> }>
                <FinishedHeader width=width/>
            </Show>
            <Show when=move || !c.room.with(RoomId::is_activity)>
                <RoomPills/>
                <QuickAdd/>
                <Show when=move || c.tomorrow.with(Option::is_some) fallback=|| view! { <TaskList/> }>
                    <Handover/>
                </Show>
                <CompletedRows/>
            </Show>
            <For each=move || c.room.with(|r| match r {
                RoomId::Activity(agent) => vec![agent.clone()],
                _ => Vec::new(),
            }) key=Clone::clone children=move |agent| view! { <activity::Feed agent=agent reached_bottom=reached_bottom/> }/>
        </div>
    }
}

#[component]
fn RoomHeader(width: RwSignal<f64>) -> impl IntoView {
    let c = ctx();
    // The shared header counts completing rows as open until they leave.
    let counts = Memo::new(move |_| {
        c.view_room
            .with(|r| (r.header.progress_done, r.header.progress_total))
    });
    view! {
        <header class="room-head">
            <div class="head-text">
                <div class="eyebrow">
                    <EyebrowLine/>
                </div>
                <div class="title-row">
                    <div class="room-title-group"><RoomTitle/><room_actions::RoomMenuButton/></div>
                    <p class="summary">
                        {move || match c.view_room.with(|r| r.header.summary) {
                            // Activity keeps no score.
                            Summary::None => ().into_any(),
                            empty @ Summary::Empty { .. } => {
                                view! { <span class="fig">{empty.text()}</span> }.into_any()
                            }
                            Summary::Done { done } => view! { <b>{done}</b>" done" }.into_any(),
                            Summary::AllClear { done, total } => view! {
                                <b>{done}</b>
                                {format!(" of {total} done")}
                                <span class="dot-sep">"·"</span>
                                <span class="finished-clear fig">"all clear"</span>
                            }
                                .into_any(),
                            Summary::Progress { done, total, open } => view! {
                                <b>{done}</b>
                                {format!(" of {total} done")}
                                <span class="dot-sep">"·"</span>
                                <span class="fig">{open}</span>
                                " open"
                            }
                                .into_any(),
                        }}
                    </p>
                </div>
            </div>
            <LandscapeStrip counts=counts width=width/>
        </header>
    }
}

/// The line above the title: an accented lead, then the details in mono caps.
#[component]
fn EyebrowLine() -> impl IntoView {
    let c = ctx();
    move || {
        let e = c.view_room.with(|r| r.header.eyebrow.clone());
        let lead = e.lead().to_owned();
        let details: Vec<String> = e.details().into_iter().map(str::to_owned).collect();
        view! {
            <span class="eyebrow-lead">{lead}</span>
            {details.into_iter().map(|m| view! { <span class="eyebrow-meta">{m}</span> }).collect_view()}
        }
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
        let now = if c.live {
            let now = platform::now();
            c.now.set(now);
            now
        } else {
            c.now.get_untracked()
        };
        let Some(room) = c
            .view_room
            .with_untracked(|r| r.add_placeholder.as_ref().map(|_| r.id.clone()))
        else {
            return;
        };
        let text = value.get_untracked();
        let (id, commands) = c.state.with_untracked(|state| {
            intent::quick_add_task(state, &room, &text, &now.eval(), backend::new_uuid)
        });
        if id.is_some() && commands.into_iter().all(|command| c.submit(command)) {
            value.set(String::new());
        }
    };
    let add_placeholder = Memo::new(move |_| c.view_room.with(|r| r.add_placeholder.clone()));
    let date = Memo::new(move |_| value.with(|text| model::date_preview(text, &c.now.get())));
    // Keep the last label available while the visual chip fades out.
    let chip_label = Memo::new(move |prev: Option<&String>| {
        date.get().or_else(|| prev.cloned()).unwrap_or_default()
    });
    view! {
        <Show when=move || add_placeholder.with(Option::is_some)>
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
                    placeholder=move || add_placeholder.get().unwrap_or_default()
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
                <span class="qa-date-announcement" role="status">{move || date.get().map(|date| format!("Due {date}"))}</span>
                <span class="qa-date-chip" class:no-date=move || date.with(Option::is_none) aria-hidden="true">
                    {icon(Icon::Calendar)}{move || chip_label.get()}
                </span>
                <span class="qa-hints" aria-hidden="true" hidden=move || c.read_only()>
                    <span class="keycap">"N"</span>
                </span>
            </form>
        </Show>
    }
}

fn rows_with_exits(room: &Room, section: RowSection, lingering: &[Linger]) -> Vec<Shown> {
    let rows = if section == RowSection::Completed {
        &room.completed
    } else {
        &room.rows
    };
    let mut shown: Vec<_> = rows
        .iter()
        .cloned()
        .map(|row| {
            // The shared room supplies membership and order for held completions.
            let phase = (section == RowSection::Open && row.done)
                .then(|| {
                    lingering.iter().find(|l| {
                        l.section == section
                            && l.room == room.id
                            && l.row.id == row.id
                            && l.phase != Phase::Depart
                    })
                })
                .flatten()
                .map(|l| l.phase);
            Shown { row, phase }
        })
        .collect();
    for l in lingering
        .iter()
        .filter(|l| l.section == section && l.room == room.id && l.phase == Phase::Depart)
    {
        if !shown.iter().any(|s| s.row.id == l.row.id) {
            shown.insert(
                l.index.min(shown.len()),
                Shown {
                    row: l.row.clone(),
                    phase: Some(l.phase),
                },
            );
        }
    }
    shown
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
            <For each=move || c.shown.get() key=|s| (row_key(&s.row), s.phase) let:s>
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
            c.lingering
                .with(|l| rows_with_exits(r, RowSection::Completed, l))
        })
    });
    view! {
        <ul id="completed-rows" class="rows completed-rows" role="list" aria-label="Completed tasks"
            hidden=move || !c.completed_open.get() || completed.with(Vec::is_empty)>
            <For each=move || if c.completed_open.get() { completed.get() } else { Vec::new() }
                key=|s| (row_key(&s.row), s.phase) let:s>
                <TaskRow row=s.row phase=s.phase/>
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
            <Show when=move || !c.detail_modal.get()>
                <ToastView/>
            </Show>
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

// Priority is read reactively. Do not replace the row on an Important toggle: that would lose
// keyboard focus and the transition's starting frame.
fn row_key(row: &Row) -> Row {
    Row {
        priority: 0,
        ..row.clone()
    }
}

#[component]
fn TaskRow(row: Row, phase: Option<Phase>) -> impl IntoView {
    let c = ctx();
    let id = row.id;
    let r = row.clone();
    let priority_at_mount = row.priority;
    let priority = move || {
        c.state.with(|s| {
            s.tasks
                .get(&id)
                .map_or(priority_at_mount, |t| model::priority_marks(t.priority))
        })
    };
    let important = move || priority() == model::priority_marks(Priority::High);
    let important_label = format!("Important: {}", row.title);
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
            class:exiting=Phase::exiting(phase)
            inert=phase == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
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
                <button type="button" class="row-title" id=format!("row-{id}")
                    aria-current=move || (c.selected.get() == Some(id)).then_some("true")>{row.title.clone()}</button>
                <span class="meta">
                    <span class="due-slot">{due_view("due")}</span>
                    {row.chip.clone().map(|ch| chip(ch.label, ch.hue))}
                    {row.steps.map(|(d, t)| view! { <span class="steps">{bars(d, t)}{format!("{d}/{t}")}</span> })}
                    {row.email.then(|| view! { <span class="meta-icon" title="From an email" aria-label="From an email" role="img">{icon(Icon::Mail)}</span> })}
                    {(!tags.is_empty()).then(|| view! { <span class="tag" title=full_tags>{tags}</span> })}
                </span>
            </div>
            <span class="col-agent">{row.agent.as_ref().map(mark)}</span>
            <span class="col-priority" data-p=move || priority().to_string()
                aria-label=move || (priority() > 0).then(|| format!("Priority {}", priority()))>
                {move || "!".repeat(usize::from(priority()))}
            </span>
            <span class="col-due">{due_view("due")}</span>
            <button type="button" class="icon-button task-toggle important-toggle row-important"
                aria-pressed=move || important().to_string() aria-label=important_label
                title=move || if important() { "Remove importance" } else { "Mark as important" }
                disabled=move || c.read_only() || phase.is_some()
                on:click=move |ev| {
                    ev.stop_propagation();
                    c.submit_intent(|state| intent::toggle_important(state, id));
                }>
                {toggle_icon(Icon::Star, Icon::StarFilled)}
            </button>
        </li>
    }
}

#[component]
fn ToastView() -> impl IntoView {
    let c = ctx();
    let toast = Memo::new(move |_| {
        let has_alert = c
            .sync
            .with(|s| s.holding_upgrade || s.read_only || s.local_error.is_some());
        if has_alert { None } else { c.toast.get() }
    });
    let dismiss = move || {
        if c.detail_modal.get_untracked() {
            platform::focus("task-detail");
        }
        c.toast.set(None);
    };
    view! {
        // Keep the live region mounted before the toast arrives. Alerts announce themselves.
        <span class="toast-announcement" role="status" aria-live="polite" aria-atomic="true">
            {move || toast.with(|t| t.as_ref().map(|t| t.text.clone()))}
        </span>
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
            toast.get().map(|t| {
                    let undo = t.undo;
                    view! {
                        <div class="toast">
                            <span class="toast-text">{t.text}</span>
                            {undo
                                .then(|| {
                                    view! {
                                        <button
                                            class="toast-undo"
                                            on:click=move |_| {
                                                if c.detail_modal.get_untracked() {
                                                    platform::focus("task-detail");
                                                }
                                                c.undo();
                                            }
                                        >
                                            {icon(Icon::UndoTwo)}
                                            "Undo"
                                            <span class="keycap">"Ctrl Z"</span>
                                        </button>
                                    }
                                })}
                            <button class="toast-close" aria-label="Dismiss" on:click=move |_| dismiss()>
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
                    <EyebrowLine/>
                </div>
                <div class="title-row">
                    <RoomTitle/>
                    <p class="summary">
                        {move || c.view_room.with(|r| format!("{} of {} done", r.header.progress_done, r.header.progress_total))}
                        <span class="dot-sep">"·"</span>
                        <span class="finished-clear fig">"all clear"</span>
                    </p>
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
    let first = Memo::new(move |_| {
        c.tomorrow
            .with(|t| t.as_ref().and_then(|t| t.first.clone()))
    });
    let weekday = Memo::new(move |_| {
        c.tomorrow
            .with(|t| t.as_ref().map(|t| t.weekday.clone()).unwrap_or_default())
    });
    view! {
        <section class="tomorrow">
            <h2 class="caps">"When the sun comes up"</h2>
            <div class="next">
                {move || match first.get() {
                    Some((row, date)) => view! { <HandoverFirst row=row date=date/> }.into_any(),
                    None => view! {
                        <span class="sunrise">{icon(Icon::Sunrise)}</span>
                        <p class="next-text no-tomorrow">{format!("Nothing is due on {} yet.", weekday.get())}</p>
                    }.into_any(),
                }}
                <a class="button primary" href=RoomId::Smart("planned").path() on:click=link_room(c, RoomId::Smart("planned"))>
                    {move || format!("Plan {}", weekday.get())}
                </a>
            </div>
            <Show when=move || c.tomorrow.with(|t| t.as_ref().is_some_and(|t| !t.also.is_empty()))>
                <h2 class="caps">{move || format!("Also on {}", weekday.get())}</h2>
                <ul class="t-rows">
                    <For each=move || c.tomorrow.with(|t| t.as_ref().map(|t| t.also.clone()).unwrap_or_default())
                        key=|row| row.clone() let:row>
                        <HandoverRow row=row show_date=false/>
                    </For>
                </ul>
            </Show>
            <Show when=move || c.tomorrow.with(|t| t.as_ref().is_some_and(|t| !t.week.is_empty()))>
                <h2 class="caps">"Rest of the week"</h2>
                <ul class="t-rows">
                    <For each=move || c.tomorrow.with(|t| t.as_ref().map(|t| t.week.clone()).unwrap_or_default())
                        key=|row| row.clone() let:row>
                        <HandoverRow row=row show_date=true/>
                    </For>
                </ul>
                {move || c.tomorrow.with(|t| t.as_ref().filter(|t| t.more > 0).map(|t| {
                    view! { <p class="more">{format!("{} more", t.more)}</p> }
                }))}
            </Show>
        </section>
    }
}

fn handover_row_phase(leaving: &[Linger], id: TaskId) -> Option<Phase> {
    leaving
        .iter()
        .find(|x| x.section == RowSection::Handover && x.row.id == id)
        .map(|x| x.phase)
}

fn handover_phase(c: Ctx, id: TaskId) -> Memo<Option<Phase>> {
    Memo::new(move |_| c.lingering.with(|l| handover_row_phase(l, id)))
}

#[component]
fn HandoverFirst(row: Row, date: String) -> impl IntoView {
    let c = ctx();
    let id = row.id;
    let phase = handover_phase(c, id);
    view! {
        <div class="next-task handover-task" class:done=row.done
            class:selected=move || c.selected.get() == Some(id)
            class:completing=move || phase.get() == Some(Phase::Hold)
            class:exiting=move || Phase::exiting(phase.get())
            inert=move || phase.get() == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
            on:click=move |_| c.select(Some(id))>
            <HandoverCheck row=row.clone() phase=phase/>
            <span class="sunrise">{icon(Icon::Sunrise)}</span>
            <div class="next-text">
                <button class="next-title" type="button" id=format!("row-{id}")>{row.title}</button>
                <div class="next-meta">
                    <span class="due" data-tone="later">{date}</span>
                    {row.chip.map(|ch| chip(ch.label, ch.hue))}
                </div>
            </div>
        </div>
    }
}

#[component]
fn HandoverRow(row: Row, show_date: bool) -> impl IntoView {
    let c = ctx();
    let id = row.id;
    let phase = handover_phase(c, id);
    view! {
        <li class="t-row handover-task" class:done=row.done
            class:selected=move || c.selected.get() == Some(id)
            class:completing=move || phase.get() == Some(Phase::Hold)
            class:exiting=move || Phase::exiting(phase.get())
            inert=move || phase.get() == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
            on:click=move |_| c.select(Some(id))>
            <HandoverCheck row=row.clone() phase=phase/>
            <button class="t-title" type="button" id=format!("row-{id}")>{row.title}</button>
            {row.chip.map(|ch| chip(ch.label, ch.hue))}
            <span class="t-date">{row.due.filter(|_| show_date).map(|d| d.text)}</span>
        </li>
    }
}

#[component]
fn HandoverCheck(row: Row, phase: Memo<Option<Phase>>) -> impl IntoView {
    let c = ctx();
    let label = format!(
        "{} “{}”",
        if row.done { "Reopen" } else { "Complete" },
        row.title
    );
    view! {
        <button class="check" type="button" role="checkbox" aria-label=label
            aria-checked=row.done.to_string() disabled=move || c.read_only()
            aria-disabled=move || phase.get().is_some().then_some("true")
            on:click=move |ev| {
                ev.stop_propagation();
                if phase.with_untracked(Option::is_none) {
                    c.toggle(&row);
                }
            }>
            {tick_svg()}
        </button>
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
    let uploads = attachments::Uploads::new(c);
    view! {
        <section class="detail" id="task-detail" tabindex="-1" aria-label="Task detail"
            role=move || c.detail_modal.get().then_some("dialog")
            aria-modal=move || c.detail_modal.get().then_some("true")
            inert=move || c.detail.with(Option::is_none)
            on:dragover=move |ev| uploads.drag_over(c, ev)
            on:drop=move |ev| uploads.drop_files(c, ev)>
            <div class="detail-scroll" on:scroll=move |_| {
                pickers::position_detail_pickers();
                c.task_actions.position();
            }>
                {move || id.get().map(|id| view! { <DetailBody id=id rendered=rendered uploads=uploads/> })}
            </div>
            // Modal feedback belongs inside the dialog, outside the inert room.
            <Show when=move || c.detail_modal.get()>
                <div class="detail-toast"><ToastView/></div>
            </Show>
        </section>
    }
}

#[component]
fn DetailBody(
    id: TaskId,
    rendered: Memo<Option<Detail>>,
    uploads: attachments::Uploads,
) -> impl IntoView {
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

            <section class="d-section steps-section">
                <DetailSteps detail=d/>
                <AddStep id=id/>
            </section>

            <section class="d-section">
                <h3 class="caps d-caps">"Notes"</h3>
                <NotesEditor id=id notes=notes/>
            </section>

            <section class="d-section" aria-label="Schedule and priority">
                <h3 class="caps d-caps">"Schedule and priority"</h3>
                <dl class="props">
                    <DueEditor id=id detail=d/>
                    <PriorityEditor id=id detail=d/>
                    <repeat::RepeatEditor id=id detail=d/>
                    <tags::TagEditor task=id detail=d/>
                    <reminder::ReminderEditor id=id detail=d/>
                </dl>
            </section>
            <links::Links task=id detail=d/>
            <attachments::Section task=id detail=d uploads=uploads/>
            {move || d.get().map(|d| view! { <DetailActivity d=d/> })}
        </div>
    }
}

#[component]
fn DetailTop(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let important =
        move || detail.with(|d| d.as_ref().is_some_and(|d| d.priority == Priority::High));
    let toggle_important = move |_| {
        c.submit_intent(|state| intent::toggle_important(state, id));
    };
    let my_day = move || detail.with(|d| d.as_ref().is_some_and(|d| d.my_day));
    let toggle_my_day = move |_| {
        let today = c.now.get_untracked().today();
        c.submit_intent(|state| intent::toggle_my_day(state, id, today));
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
                class="icon-button task-toggle my-day-toggle"
                aria-pressed=move || my_day().to_string()
                title=move || if my_day() { "Remove from My Day" } else { "Add to My Day" }
                aria-label="My Day"
                disabled=move || c.read_only()
                on:click=toggle_my_day
            >
                {toggle_icon(Icon::Sun, Icon::SunFilled)}
            </button>
            <button
                class="icon-button task-toggle important-toggle"
                aria-pressed=move || important().to_string()
                title=move || if important() { "Remove importance" } else { "Mark as important" }
                aria-label="Important"
                disabled=move || c.read_only()
                on:click=toggle_important
            >
                {toggle_icon(Icon::Star, Icon::StarFilled)}
            </button>
            <task_actions::DetailMenu task=id/>
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
    let check_row = c.actionable_rows().into_iter().find(|row| row.id == id);
    let handover = handover_phase(c, id);
    let on_check = move |_| {
        if handover.with_untracked(Option::is_some) {
            return;
        }
        if done {
            c.uncomplete(id);
        } else if let Some(r) = check_row.clone() {
            c.complete(&r);
        } else {
            c.submit_intent(|state| intent::set_done(state, id, true));
        }
    };
    view! {
        <button
            class="check big"
            role="checkbox"
            disabled=move || c.read_only()
            aria-checked=done.to_string()
            aria-label=if done { "Reopen" } else { "Complete" }
            aria-disabled=move || handover.get().is_some().then_some("true")
            on:click=on_check
        >
            {tick_svg()}
        </button>
    }
}

#[component]
fn DetailSteps(detail: Memo<Option<Detail>>) -> impl IntoView {
    let steps = Memo::new(move |_| {
        detail.with(|d| d.as_ref().map(|d| d.steps.clone()).unwrap_or_default())
    });
    let counts = Memo::new(move |_| steps.with(|s| (s.iter().filter(|s| s.done).count(), s.len())));
    view! {
        <h3 class="caps d-caps">
            "Steps"
            {move || {
                let (done, total) = counts.get();
                (total > 0).then(|| view! { <span class="steps">{bars(done, total)}{format!("{done}/{total}")}</span> })
            }}
        </h3>
        <ul class="step-rows">
            <For each=move || steps.with(|s| s.iter().map(|s| s.id).collect::<Vec<_>>())
                key=|id| *id let:id>
                <DetailStep id=id steps=steps/>
            </For>
        </ul>
    }
}

#[component]
fn DetailStep(id: StepId, steps: Memo<Vec<model::StepView>>) -> impl IntoView {
    let c = ctx();
    let step = Memo::new(move |_| {
        steps.with(|s| {
            s.iter()
                .enumerate()
                .find(|(_, step)| step.id == id)
                .map(|(i, step)| (i, step.clone()))
        })
    });
    let index = move || step.with(|s| s.as_ref().map_or(0, |(i, _)| *i));
    let done = move || step.with(|s| s.as_ref().is_some_and(|(_, s)| s.done));
    let title = Signal::derive(move || {
        step.with(|s| s.as_ref().map(|(_, s)| s.title.clone()).unwrap_or_default())
    });
    let editing = RwSignal::new(false);
    let draft = Draft::new(title);
    let input = NodeRef::<html::Input>::new();
    let focus_title = move || {
        platform::focus(&format!("step-title-{id}"));
    };
    let finish = move |save| {
        if editing.get_untracked() {
            editing.set(false);
            if save {
                let text = draft.text.get_untracked();
                c.submit_intent(|s| intent::retitle_step(s, id, &text));
            }
            draft.reset(title);
        }
    };
    Effect::new(move |_| {
        if editing.get()
            && let Some(input) = input.get()
        {
            let _ = input.focus();
            input.select();
        }
    });
    let arrow_id = move |by| format!("step-move-{id}-{by}");
    let move_step = move |by| {
        if let Some(cmd) = c.state.with_untracked(|s| intent::nudge(s, id.into(), by))
            && c.submit_undoable(cmd, "Step moved.".into(), None)
        {
            request_animation_frame(move || platform::refocus_row(&arrow_id(by)));
        }
    };
    view! {
        <li class="step" class:done=done>
            <button class="check small" type="button" role="checkbox"
                disabled=move || c.read_only() aria-checked=move || done().to_string()
                aria-label=move || format!("Step {}", index() + 1)
                on:click=move |_| { c.submit_intent(|state| intent::toggle_step(state, id)); }>
                {tick_svg()}
            </button>
            <Show when=move || editing.get() fallback=move || view! {
                <button type="button" class="step-title" id=format!("step-title-{id}") title="Rename step"
                    disabled=move || c.read_only() on:click=move |_| editing.set(true)><span>{move || title.get()}</span></button>
            }>
                <input class="step-input" type="text" node_ref=input aria-label="Step title"
                    disabled=move || c.read_only() prop:value=move || draft.text.get()
                    on:input=move |ev| draft.input(event_target_value(&ev))
                    on:blur=move |_| finish(true)
                    on:keydown=move |ev| {
                        if !ev.is_composing() && matches!(ev.key().as_str(), "Enter" | "Escape") {
                            ev.prevent_default();
                            ev.stop_propagation();
                            if !ev.repeat() {
                                finish(ev.key() == "Enter");
                                request_animation_frame(focus_title);
                            }
                        }
                    }/>
            </Show>
            <div class="step-tail">
                <span class="step-n" aria-hidden="true">{move || format!("{:02}", index() + 1)}</span>
                <div class="step-actions">
                    // At a boundary the arrow stays focusable so a move to that edge keeps focus.
                    <button class="icon-button" type="button" title="Move step up" id=arrow_id(-1)
                        aria-label=move || format!("Move step {} up", index() + 1)
                        disabled=move || c.read_only()
                        aria-disabled=move || (index() == 0).then_some("true")
                        on:click=move |_| move_step(-1)>{icon(Icon::ChevronUp)}</button>
                    <button class="icon-button" type="button" title="Move step down" id=arrow_id(1)
                        aria-label=move || format!("Move step {} down", index() + 1)
                        disabled=move || c.read_only()
                        aria-disabled=move || steps.with(|s| (index() + 1 >= s.len()).then_some("true"))
                        on:click=move |_| move_step(1)>{icon(Icon::ChevronDown)}</button>
                    <button class="icon-button" type="button" title="Delete step"
                        aria-label=move || format!("Delete step {}", index() + 1)
                        disabled=move || c.read_only()
                        on:click=move |_| {
                            let label = title.get_untracked();
                            if c.submit_undoable(intent::delete(id), format!("Deleted step “{label}”"), None) {
                                request_animation_frame(|| platform::refocus_row("add-step"));
                            }
                        }>{icon(Icon::TrashTwo)}</button>
                </div>
            </div>
        </li>
    }
}

#[component]
fn DueEditor(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let due = Memo::new(move |_| detail.with(|d| d.as_ref().and_then(|d| d.due.clone())));
    let editing = RwSignal::new(false);
    let invalid = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let input = NodeRef::<html::Input>::new();
    let start = move |_| {
        draft.set(due.with_untracked(|d| {
            d.as_ref()
                .map(|(date, _)| date.to_string())
                .unwrap_or_default()
        }));
        invalid.set(false);
        editing.set(true);
    };
    let close = move || {
        editing.set(false);
        request_animation_frame(|| {
            platform::focus("edit-task-due");
        });
    };
    let save = move || {
        let now = if c.live {
            platform::now()
        } else {
            c.now.get_untracked()
        };
        let Ok(date) = model::due_edit(&draft.get_untracked(), &now) else {
            invalid.set(true);
            return;
        };
        let cmd = c.state.with_untracked(|s| intent::set_due(s, id, date));
        if cmd.is_none_or(|cmd| c.submit(cmd)) {
            close();
        }
    };
    Effect::new(move |_| {
        if editing.get()
            && let Some(input) = input.get()
        {
            let _ = input.focus();
            input.select();
        }
    });
    view! {
        <div class="prop">
            <dt>{icon(Icon::Calendar)}"Due"</dt>
            <dd class="mono">
                <Show when=move || editing.get() fallback=move || view! {
                    <button id="edit-task-due" type="button" class="prop-edit"
                        aria-label=move || format!("Edit due date: {}", due.with(|d| d.as_ref().map(|(_, label)| label.clone()).unwrap_or_else(|| "not set".into())))
                        title="Edit due date (D)" aria-keyshortcuts="D" disabled=move || c.read_only()
                        on:click=start>
                        {move || due.with(|d| d.as_ref().map(|(_, label)| label.clone()).unwrap_or_else(|| "Set date".into()))}
                    </button>
                }>
                    <input class="prop-input" type="text" node_ref=input aria-label="Due date"
                        disabled=move || c.read_only() placeholder="tomorrow, fri, +3d…"
                        aria-invalid=move || invalid.get().to_string()
                        aria-describedby=move || invalid.get().then_some("due-date-error")
                        prop:value=move || draft.get()
                        on:input=move |ev| { draft.set(event_target_value(&ev)); invalid.set(false); }
                        on:blur=move |_| editing.set(false)
                        on:keydown=move |ev| {
                            if !ev.is_composing() && matches!(ev.key().as_str(), "Enter" | "Escape") {
                                ev.prevent_default();
                                ev.stop_propagation();
                                if !ev.repeat() {
                                    if ev.key() == "Enter" { save(); } else { close(); }
                                }
                            }
                        }/>
                    <Show when=move || invalid.get()>
                        <p id="due-date-error" class="prop-error" role="alert">"Try tomorrow, fri, +3d or a date like 2026-12-01."</p>
                    </Show>
                </Show>
            </dd>
        </div>
    }
}

#[component]
fn PriorityEditor(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let priority =
        Memo::new(move |_| detail.with(|d| d.as_ref().map_or(Priority::None, |d| d.priority)));
    let return_focus = move || {
        if c.detail_modal.get_untracked() || !platform::focus(&format!("row-{id}")) {
            platform::focus("task-detail");
        }
    };
    let options = [
        (Priority::None, "None", "None"),
        (Priority::Low, "Low", "! Low"),
        (Priority::Normal, "Normal", "!! Normal"),
        (Priority::High, "High", "!!! High"),
    ];
    view! {
        <div class="prop">
            <dt>{icon(Icon::Flag)}<label for="task-priority">"Priority"</label></dt>
            <dd class="mono">
                <select id="task-priority" class="prop-input" title="Choose priority (Shift+P)" aria-keyshortcuts="Shift+P"
                    disabled=move || c.read_only()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        if let Some((level, _, _)) = options.iter().find(|(_, name, _)| value == *name) {
                            c.submit_intent(|s| intent::pick_priority(s, id, *level));
                        }
                    }
                    on:keydown=move |ev| {
                        if ev.repeat() && matches!(ev.key().as_str(), "Enter" | "Escape" | "ArrowUp" | "ArrowDown" | " ") {
                            ev.prevent_default();
                            ev.stop_propagation();
                        } else if ev.key() == "Escape" {
                            ev.stop_propagation();
                            return_focus();
                        }
                    }>
                    {options.into_iter().map(|(level, name, label)| view! {
                        <option value=name prop:selected=move || priority.get() == level>{label}</option>
                    }).collect_view()}
                </select>
            </dd>
        </div>
    }
}

#[component]
fn DetailActivity(d: Detail) -> impl IntoView {
    view! {
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
        let text = draft.text.get_untracked();
        draft.dirty.set(false);
        c.submit_intent(|state| intent::retitle(state, id, &text));
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
                            if ev.is_composing() { return; }
                            if ev.repeat() && matches!(ev.key().as_str(), "Enter" | "Escape") {
                                ev.prevent_default();
                                ev.stop_propagation();
                                return;
                            }
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
                let text = draft.text.get_untracked();
                draft.dirty.set(false);
                c.submit_intent(|state| intent::set_notes(state, id, &text));
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
                let title = value.get_untracked();
                if let Some(cmd) = intent::add_step(id, &title, backend::new_uuid)
                    && c.submit(cmd)
                {
                    value.set(String::new());
                }
            }
        >
            {icon(Icon::Plus)}
            <input
                type="text"
                id="add-step"
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
                {move || c.room.with(|room| match room {
                    RoomId::Activity(agent) => Some(c.activity_feed.with(|f| {
                        f.counts(agent.as_ref(), &c.now.with(Now::local_time)).text()
                    })),
                    _ => c.view_room.with(|r| r.header.status.map(|s| s.text())),
                })}
            </span>
            <span class="sb-keys" aria-hidden="true">
                <Show when=move || !c.shown.with(Vec::is_empty) || c.tomorrow.with(|t| t.as_ref().is_some_and(|t| t.rows().any(|r| !r.done)))>
                    <span><kbd class="keycap">"J"</kbd><kbd class="keycap">"K"</kbd>" move"</span>
                </Show>
                <Show when=move || !c.read_only() && c.selected.with(|id| id.is_some_and(|id|
                    c.shown.with(|rows| rows.iter().any(|s| s.row.id == id && s.phase.is_none()))
                    || c.tomorrow.with(|t| t.as_ref().is_some_and(|t| t.rows().any(|r| r.id == id && !r.done
                        && c.lingering.with(|l| handover_row_phase(l, id).is_none()))))))>
                    <span><kbd class="keycap">"Space"</kbd>" toggle completion"</span>
                </Show>
                <Show when=move || !c.read_only() && c.detail.with(Option::is_some)>
                    <span><kbd class="keycap">"E"</kbd>" edit title"</span>
                </Show>
                <Show when=move || !c.detail_modal.get() && !c.read_only() && c.view_room.with(|r| r.add_placeholder.is_some())>
                    <span><kbd class="keycap">"N"</kbd>" add task"</span>
                </Show>
                <Show when=move || !c.read_only() && c.undo.with(Option::is_some)>
                    <span><kbd class="keycap">"Ctrl Z"</kbd>" undo"</span>
                </Show>
                <Show when=move || c.detail.with(Option::is_some)>
                    <span><kbd class="keycap">"Esc"</kbd>" close detail"</span>
                </Show>
            </span>
        </footer>
    }
}
