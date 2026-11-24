//! The Leptos components. Layout and look are in `static/app.css`, which takes every value from
//! `tokens.css`; the view supplies state, including the user's preferred docked detail width.

mod activity;
mod assistant;
mod assistant_provider;
mod attachments;
mod board;
mod bulk;
mod detail_splitter;
mod device_pairing;
mod email_drop;
mod links;
mod navigation;
mod pickers;
mod quick_add;
mod reminder;
mod reminder_alerts;
mod repeat;
mod room_actions;
mod sidebar_drag;
mod tags;
mod task_actions;
mod task_drag;
mod timeline;
mod trash;
mod workflow;

use crate::backend::{self, Sync, UndoTarget};
use crate::changes::{self, Failure};
use crate::landscape::{self, Sky};
use crate::model::{
    self, AgentMark, Detail, DueTone, Now, Room, RoomId, Row, Sidebar, Source, Tomorrow,
};
use crate::platform;
use leptos::html;
use leptos::prelude::*;
use quick_add::{CalendarPopover, DatePicker, QuickAdd};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use todo_client::room_header::Summary;
use todo_client::{CommandId, Connection, SubmitError, TrackedUndo, intent};
use todo_core::ids::{CollectionId, ListId, StepId, TaskId};
use todo_core::model::{ListFeatures, ListView, Priority, Stamp};
use todo_core::query::QueryStatus;
use todo_core::{Actor, Command, State};
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
    board_slot: Option<(QueryStatus, usize)>,
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
    targets: Vec<UndoTarget>,
    label: String,
    animated_task: Option<TaskId>,
}

#[derive(Clone)]
struct WaitingUndo {
    pending: Vec<CommandId>,
    label: String,
    error: Option<String>,
}

/// The toast at the bottom of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    id: u64,
    text: String,
    undo: bool,
    sidebar: bool,
}

/// Shared UI state and whether it follows the real clock (`false` for fixtures).
#[derive(Clone, Copy)]
pub struct Ctx {
    state: RwSignal<Arc<State>>,
    sync: RwSignal<Sync>,
    ai: assistant::Ui,
    sign_in_started: StoredValue<bool>,
    synced_at: RwSignal<Option<Now>>,
    now: RwSignal<Now>,
    live: bool,
    room: RwSignal<RoomId>,
    selected: RwSignal<Option<TaskId>>,
    history: StoredValue<navigation::BrowserHistory>,
    multi: RwSignal<todo_client::selection::Selection>,
    bulk_due: NodeRef<html::Dialog>,
    lingering: RwSignal<Vec<Linger>>,
    toast: RwSignal<Option<Toast>>,
    reminder_alerts: reminder_alerts::Alerts,
    toast_seq: RwSignal<u64>,
    undo: RwSignal<Option<Undo>>,
    waiting_undo: RwSignal<BTreeMap<u64, WaitingUndo>>,
    activity_undos: activity::Undos,
    activity_feed: RwSignal<activity::FeedState>,
    trash: Memo<Vec<todo_client::trash::Row>>,
    activity_revision: RwSignal<Option<(Option<todo_client::Epoch>, u64)>>,
    room_actions: room_actions::Controls,
    device_settings_open: RwSignal<bool>,
    task_actions: task_actions::Controls,
    task_drag: RwSignal<Option<task_drag::Gesture>>,
    sidebar_drag: RwSignal<Option<sidebar_drag::Gesture>>,
    sidebar_click_blocked: RwSignal<bool>,
    board_ui: board::Ui,
    board: Memo<Option<todo_client::board::Board>>,
    timeline_ui: timeline::Ui,
    timeline: Memo<Option<todo_client::timeline::Timeline>>,
    status_menu: workflow::StatusMenu,
    room_features: Memo<ListFeatures>,
    room_view: Memo<ListView>,
    view_override: RwSignal<Option<ListView>>,
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
        self.ai.sync(s.signed_out);
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
        for (seq, mut undo) in self.waiting_undo.get_untracked() {
            let count = undo.pending.len();
            undo.pending.retain(|&id| match backend::undo_result(id) {
                None => true,
                Some(result) => {
                    if let Err(error) = result
                        && undo.error.is_none()
                    {
                        undo.error = Some(error.into());
                    }
                    false
                }
            });
            if undo.pending.is_empty() {
                self.waiting_undo.update(|pending| {
                    pending.remove(&seq);
                });
                // A late answer must not take the Undo button away from a newer change.
                if seq == self.toast_seq.get_untracked() {
                    let label = match undo.error {
                        Some(message) => format!("{message} ({})", undo.label),
                        None => format!("Undid: {}", undo.label),
                    };
                    self.show_toast(label, false);
                }
            } else if undo.pending.len() != count {
                self.waiting_undo.update(|pending| {
                    pending.insert(seq, undo);
                });
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
        self.remember_undo(vec![target], label, animated_task);
        true
    }

    fn remember_undo(self, targets: Vec<UndoTarget>, label: String, animated_task: Option<TaskId>) {
        self.undo.set(Some(Undo {
            targets,
            animated_task,
            label: label.clone(),
        }));
        self.show_toast(label, true);
    }

    fn undo(self) {
        let Some(action) = self.undo.try_update(Option::take).flatten() else {
            return;
        };
        let board_focus = pickers::focused()
            .filter(|el| el.class_list().contains("board-title"))
            .and_then(|el| el.id().strip_prefix("row-").and_then(|id| id.parse().ok()));
        self.toast.set(None);
        let mut waiting = WaitingUndo {
            pending: Vec::new(),
            label: action.label,
            error: None,
        };
        let mut applied = false;
        // An email drop saves several commands. Undo dependents, including email links, first.
        for target in action.targets.iter().rev() {
            let error = match backend::undo(target) {
                Ok(TrackedUndo::Cancelled) => {
                    applied = true;
                    None
                }
                Ok(TrackedUndo::Queued(id)) => {
                    applied = true;
                    waiting.pending.push(id);
                    None
                }
                Ok(TrackedUndo::Unavailable) => Some(changes::undo_failure("undo_no_event").into()),
                Err(message) => Some(message),
            };
            waiting.error = waiting.error.or(error);
        }
        if applied && let Some(id) = action.animated_task {
            self.lingering.update(|l| l.retain(|x| x.row.id != id));
        }
        if waiting.pending.is_empty() {
            self.show_toast(
                waiting.error.unwrap_or_else(|| "Change undone.".into()),
                false,
            );
        } else {
            let seq = self.toast_seq.get_untracked();
            self.waiting_undo.update(|pending| {
                pending.insert(seq, waiting);
            });
        }
        self.refresh();
        if applied {
            if let Some(id) = board_focus {
                board::refocus(self, id);
            } else if untrack(|| self.on_timeline()) {
                timeline::after_undo(self);
            }
        }
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
        self.toast.set(Some(Toast {
            id,
            text,
            undo,
            sidebar: false,
        }));
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
        self.cancel_task_drag();
        self.cancel_sidebar_drag();
        self.board_ui.reset_view();
        self.timeline_ui.reset();
        self.view_override.set(None);
        self.status_menu.close(false);
        self.multi.update(todo_client::selection::Selection::clear);
        self.close_bulk_due(false);
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
        self.navigate(room, None);
    }

    fn navigate(self, room: RoomId, task: Option<TaskId>) {
        self.reset_room_controls();
        self.room.set(room);
        self.set_detail(task);
        self.record_location();
    }

    fn select(self, id: Option<TaskId>) {
        self.preview_task(id);
        self.record_location();
    }

    fn preview_task(self, id: Option<TaskId>) {
        self.multi.update(todo_client::selection::Selection::clear);
        self.set_detail(id);
    }

    fn set_detail(self, id: Option<TaskId>) {
        self.editing.set(false);
        self.selected.set(id);
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
        let board_slot = untrack(|| self.on_board())
            .then(|| board::slot(self, id))
            .flatten();
        let timeline_slot = untrack(|| self.on_timeline())
            .then(|| timeline::slot(self, id))
            .flatten();
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
                board_slot,
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
                    move || {
                        let timeline_focused = timeline_slot.is_some()
                            && untrack(|| self.on_timeline())
                            && pickers::focused().is_some_and(|el| el.id() == format!("row-{id}"));
                        self.lingering.update(|l| l.retain(|x| x.token != token));
                        if timeline_focused && let Some(slot) = timeline_slot {
                            timeline::after_completion(self, id, slot);
                        }
                        if board_slot.is_some()
                            && pickers::focused().is_some_and(|el| {
                                el.closest("[data-board-card]")
                                    .ok()
                                    .flatten()
                                    .is_some_and(|card| {
                                        card.get_attribute("data-board-card").as_deref()
                                            == Some(id.to_string().as_str())
                                    })
                            })
                        {
                            board::refocus(self, id);
                        }
                    },
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
            .multi
            .with_untracked(todo_client::selection::Selection::cursor)
            .or(self.selected.get_untracked())
            .and_then(|id| rows.iter().position(|r| r.id == id));
        let next = match cur {
            None => 0,
            Some(i) => i
                .saturating_add_signed(delta)
                .min(rows.len().saturating_sub(1)),
        };
        if let Some(row) = rows.get(next) {
            self.preview_task(Some(row.id));
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

#[component]
fn App(now: Now, live: bool, notice: Option<String>) -> impl IntoView {
    let state = RwSignal::new(backend::view());
    let (history, location, view0) =
        state.with_untracked(|s| navigation::BrowserHistory::start(s, &platform::path()));
    let navigation::Location {
        room: room0,
        task: task0,
        ..
    } = location;
    let now = RwSignal::new(Now {
        date_format: platform::load_date_format(),
        ..now
    });
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
    let view_override = RwSignal::new(view0);
    let room_features = Memo::new(move |_| room.with(|r| state.with(|s| workflow::features(s, r))));
    let room_view =
        Memo::new(move |_| workflow::effective(room_features.get(), view_override.get()));
    let board = Memo::new(move |_| {
        (room_view.get() == ListView::Board)
            .then(|| view_room.with(|r| state.with(|s| board::projection(s, r, now.get()))))
            .flatten()
    });
    let timeline = Memo::new(move |_| {
        (room_view.get() == ListView::Timeline)
            .then(|| {
                room.with(|r| {
                    state.with(|s| {
                        filter_text.with(|text| {
                            holding.with(|held| model::timeline_room(s, r, now.get(), text, held))
                        })
                    })
                })
            })
            .flatten()
    });
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
    let viewport = RwSignal::new(platform::viewport_width());
    let ai = assistant::Ui::new(viewport, narrow);
    let beside_ai = Memo::new(move |_| ai.open.get() && !narrow.get() && !ai.four_wide.get());
    let detail_modal = Memo::new(move |_| {
        !ai.modal.get() && (narrow.get() || beside_ai.get()) && detail.with(Option::is_some)
    });
    let c = Ctx {
        state,
        sync: RwSignal::new(backend::status()),
        ai,
        sign_in_started: StoredValue::new(false),
        synced_at: RwSignal::new(None),
        now,
        live,
        room,
        selected,
        history: StoredValue::new(history),
        multi: RwSignal::new(todo_client::selection::Selection::default()),
        bulk_due: NodeRef::new(),
        lingering,
        toast: RwSignal::new(None),
        reminder_alerts: reminder_alerts::Alerts::new(),
        toast_seq: RwSignal::new(0),
        undo: RwSignal::new(None),
        waiting_undo: RwSignal::new(BTreeMap::new()),
        activity_undos: activity::Undos::new(),
        activity_feed: RwSignal::new(activity::FeedState::default()),
        activity_revision: RwSignal::new(None),
        trash: Memo::new(move |_| {
            if room.with(|r| *r == RoomId::Trash) {
                state.with(|s| todo_client::trash::rows(s, now.get().at))
            } else {
                Vec::new()
            }
        }),
        room_actions: room_actions::Controls::new(),
        device_settings_open: RwSignal::new(false),
        task_actions: task_actions::Controls::new(),
        task_drag: RwSignal::new(None),
        sidebar_drag: RwSignal::new(None),
        sidebar_click_blocked: RwSignal::new(false),
        board_ui: board::Ui::new(),
        board,
        timeline_ui: timeline::Ui::new(now.get_untracked().today()),
        timeline,
        status_menu: workflow::StatusMenu::new(),
        room_features,
        room_view,
        view_override,
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
    Effect::new(move |_| c.room.with(|r| c.board_ui.load_rail(r)));
    Effect::new(move |previous: Option<ListView>| {
        let view = c.room_view.get();
        if previous.is_some_and(|old| old != view) {
            c.board_ui.reset_view();
            c.status_menu.close(false);
        }
        view
    });
    Effect::new(move |_| {
        let visible = c.bulk_visible();
        c.multi.update(|s| s.retain_visible(&visible));
    });
    c.refresh();
    c.reconcile_location();
    if let Some(n) = notice {
        c.show_toast(n, false);
    }
    if live || (cfg!(feature = "fixtures") && platform::query().contains("fixture=reminder-alerts"))
    {
        reminder_alerts::start(c);
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

    platform::on_pop_state(move |marker| c.pop_location(marker.as_deref()));

    let _keys = window_event_listener(leptos::ev::keydown, move |ev| on_key(c, &ev));
    let _drag_blur = window_event_listener(leptos::ev::blur, move |_| {
        c.cancel_task_drag();
        c.cancel_sidebar_drag();
        c.timeline_ui.cancel();
    });
    let _resize = window_event_listener(leptos::ev::resize, move |_| {
        viewport.set(platform::viewport_width())
    });
    let _ai_settings = window_event_listener(leptos::ev::storage, move |ev| {
        ai.storage_changed(ev.key().as_deref());
    });
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
                c.reconcile_location();
            }
        });
        platform::on_wake(move || {
            c.now.update(|now| {
                *now = Now {
                    date_format: now.date_format,
                    ..platform::now()
                }
            });
            backend::nudge();
        });
    }
    if now.with_untracked(|n| n.browser_time) {
        set_interval(
            move || {
                c.now.update(|now| {
                    *now = Now {
                        date_format: now.date_format,
                        ..platform::now()
                    }
                })
            },
            Duration::from_secs(30),
        );
    }

    track_overlay_focus(c);
    use detail_splitter::{CHROME, Layout, Pane};
    use todo_design::layout::{desktop, elevation};
    let bulk_summary = Memo::new(move |_| !narrow.get() && !beside_ai.get() && c.multi_active());
    let docked = Memo::new(move |_| {
        !narrow.get() && !beside_ai.get() && (detail.with(Option::is_some) || bulk_summary.get())
    });
    let ai_docked = Memo::new(move |_| ai.open.get() && !narrow.get());
    let detail_preferred = Layout::preferred(Pane::Detail);
    let ai_layout = Layout::new(
        Pane::Assistant,
        Layout::preferred(Pane::Assistant),
        viewport,
        move || {
            if docked.get() {
                detail_preferred.get().min(viewport.get()) + elevation::HAIRLINE as i32
            } else {
                0
            }
        },
    );
    let ai_reserved = move || {
        if ai_docked.get() {
            ai_layout.width.get() + elevation::HAIRLINE as i32
        } else {
            0
        }
    };
    let detail_layout = Layout::new(Pane::Detail, detail_preferred, viewport, ai_reserved);
    let compact = move || {
        let room_width = viewport.get()
            - CHROME
            - ai_reserved()
            - if docked.get() {
                detail_layout.width.get()
            } else {
                0
            };
        (docked.get() || ai_docked.get()) && room_width < desktop::LIST_MIN as i32
    };
    let app = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        let detail = docked.get().then(|| detail_layout.width.get());
        let assistant = ai_docked.get().then(|| ai_layout.width.get());
        if let Some(app) = app.get() {
            let style = web_sys::HtmlElement::style(&app);
            for (key, width, pane) in [
                ("--detail-pane-width", detail, Pane::Detail),
                ("--assistant-pane-width", assistant, Pane::Assistant),
            ] {
                if let Some(width) = width.filter(|w| *w != pane.default_width()) {
                    // Numeric CSSOM properties, not inline style markup.
                    let _ = style.set_property(key, &format!("{width}px"));
                } else {
                    let _ = style.remove_property(key);
                }
            }
            if style.length() == 0 {
                let _ = app.remove_attribute("style");
            }
        }
    });
    view! {
        <div class="app" node_ref=app class:has-detail=move || c.detail.with(Option::is_some)
            class:has-bulk-summary=move || bulk_summary.get()
            class:compact-room=compact class:assistant-open=move || ai.open.get()
            class:ai-detail-overlay=move || beside_ai.get() && c.detail_modal.get()
            class:assistant-modal=move || ai.modal.get()
            class:detail-modal=move || c.detail_modal.get() class:drawer-open=move || c.drawer.get()>
            <SidebarPane/>
            <div class="scrim" aria-hidden="true" on:click=move |_| {
                if ai.modal.get_untracked() { ai.dismiss(); }
                else if c.drawer.get_untracked() { c.drawer.set(false); } else { c.select(None); }
            }></div>
            <main class="main" inert=move || (c.drawer.get() && !c.drag_active()) || c.detail_modal.get() || ai.modal.get()>
                <AppBar/>
                <RoomPane/>
                <bulk::Bar/>
                <ListFoot/>
            </main>
            <detail_splitter::Splitter layout=detail_layout docked=docked/>
            <DetailPane/>
            <detail_splitter::Splitter layout=ai_layout docked=ai_docked/>
            <assistant::Panel/>
            <Show when=move || bulk_summary.get()>
                <aside class="bulk-summary" aria-label="Selection summary">
                    {icon(Icon::List)}
                    <p>{move || todo_client::selection::count_label(c.bulk_ids().len())}</p>
                </aside>
            </Show>
            <StatusBar/>
        </div>
        <room_actions::Overlays/>
        <task_actions::Overlays/>
        <workflow::StatusPopover/>
        <task_drag::Lift/>
        <sidebar_drag::Lift/>
        <bulk::DueDialog/>
        <assistant::Settings/>
    }
}

fn track_overlay_focus(c: Ctx) {
    Effect::new(move |prev: Option<(bool, Option<TaskId>, RoomId)>| {
        let modal = c.detail_modal.get();
        let id = c.detail.with(|d| d.as_ref().map(|d| d.id));
        let room = c.room.get();
        if modal && !prev.as_ref().is_some_and(|(open, _, _)| *open) {
            request_animation_frame(move || {
                if c.detail_modal.get_untracked()
                    && !(c.ai.open.get_untracked()
                        && pickers::focused().is_some_and(|el| el.id() == "assistant-prompt"))
                {
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
                    && !untrack(|| c.multi_active())
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
                .is_some_and(|(_, old)| old != &room && (old.is_history() || room.is_history()))
        {
            request_animation_frame(|| {
                platform::focus("room-title");
            });
        } else if open || prev.is_some_and(|(was_open, _)| was_open) {
            request_animation_frame(move || {
                if open != c.drawer.get_untracked() {
                    return;
                }
                if open {
                    // A control may already have focus by the time this frame runs.
                    if !document()
                        .active_element()
                        .is_some_and(|el| el.closest("#list-drawer").ok().flatten().is_some())
                    {
                        platform::focus("list-drawer");
                    }
                } else {
                    platform::focus("open-drawer");
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
    if key == "escape" && c.cancel_sidebar_drag() {
        ev.prevent_default();
        return;
    }
    if untrack(|| {
        c.sidebar_active() || c.drag_active() || c.board_ui.dragging() || c.timeline_ui.dragging()
    }) {
        if key == "escape" {
            c.cancel_task_drag();
            c.board_ui.cancel();
            c.timeline_ui.cancel();
            ev.prevent_default();
        }
        return;
    }
    if assistant::key(c, ev, &key) {
        return;
    }
    let drawer = c.drawer.get_untracked();
    let modal = c.detail_modal.get_untracked();
    if key == "tab" && (drawer || modal && !c.ai.open.get_untracked()) {
        platform::contain_tab(if drawer { "list-drawer" } else { "task-detail" }, ev);
        return;
    }
    let typing = typing(ev);
    let timeline = untrack(|| c.on_timeline());
    if !typing
        && !drawer
        && !timeline
        && !c.room.with_untracked(RoomId::is_history)
        && bulk::key(c, ev, &key)
    {
        return;
    }
    if ev.repeat()
        && !(!typing
            && !drawer
            && !modal
            && !ev.alt_key()
            && !ev.ctrl_key()
            && !ev.meta_key()
            && !ev.shift_key()
            && ((timeline && matches!(key.as_str(), "j" | "k" | "arrowup" | "arrowdown"))
                || (untrack(|| c.on_board())
                    && matches!(
                        key.as_str(),
                        "h" | "l"
                            | "j"
                            | "k"
                            | "arrowleft"
                            | "arrowright"
                            | "arrowup"
                            | "arrowdown"
                    ))))
    {
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
            if timeline && !modal {
                timeline::clear_cursor(c);
            }
        } else if c.filter_open.get_untracked() {
            c.close_filter();
        } else if timeline && !typing {
            timeline::clear_cursor(c);
        }
        return;
    }
    let history = c.room.with_untracked(RoomId::is_history);
    if !typing
        && !history
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
        if !drawer
            && !modal
            && !history
            && ev.alt_key()
            && !ev.shift_key()
            && matches!(key.as_str(), "arrowleft" | "arrowright")
            && ((timeline && timeline::key(c, ev, &key))
                || (untrack(|| c.on_board()) && board::key(c, ev, &key)))
        {
            ev.prevent_default();
        } else if key == "z"
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
    if drawer || history {
        // The history has no task selection or composer. Keep native scrolling and button keys.
        return;
    }
    if (!modal && timeline && timeline::key(c, ev, &key))
        || workflow::key(c, ev, &key, modal)
        || (!modal && untrack(|| c.on_board()) && board::key(c, ev, &key))
    {
        ev.prevent_default();
        return;
    }
    if ev.alt_key() {
        if !ev.shift_key()
            && !timeline
            && !untrack(|| c.on_board())
            && !c.read_only()
            && matches!(key.as_str(), "arrowup" | "arrowdown")
            && c.room.with_untracked(|r| matches!(r, RoomId::List(_)))
        {
            ev.prevent_default();
            c.nudge_task(key == "arrowdown", modal);
        }
        return;
    }
    if !c.read_only() && task_actions::shortcut(c, ev) {
        return;
    }
    match key.as_str() {
        "j" | "k" | "arrowup" | "arrowdown" if timeline || untrack(|| c.on_board()) => {}
        "j" | "arrowdown" => {
            ev.prevent_default();
            c.step_selection(1);
        }
        "k" | "arrowup" => {
            ev.prevent_default();
            c.step_selection(-1);
        }
        " " if !timeline && !c.read_only() => {
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
        "d" | "p" if !c.read_only() && !(key == "d" && ev.shift_key()) => {
            let Some(id) = c.detail.with_untracked(|d| d.as_ref().map(|d| d.id)) else {
                return;
            };
            ev.prevent_default();
            if key == "d" {
                if platform::focus(DUE_ANCHOR)
                    && let Some(button) = document()
                        .get_element_by_id(DUE_ANCHOR)
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
            <span class="chip-label">{label}</span>
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

fn sync_presentation(
    s: &Sync,
    at: Option<Now>,
) -> (todo_client::sync_label::SyncText, &'static str) {
    use todo_client::sync_label::{SyncLabel, SyncTone};

    let time = at.map(|n| {
        let l = n.local();
        format!("{:02}:{:02}", l.hour(), l.minute())
    });
    let label = SyncLabel::for_client(
        Some(s.connection),
        s.pending,
        s.failed,
        s.signed_out,
        time.as_deref(),
    );
    let dot = match label.tone() {
        SyncTone::Ok => "online",
        SyncTone::Busy => "syncing",
        SyncTone::Idle => "offline",
        SyncTone::Problem => "error",
    };
    (label.text(), dot)
}

// ---- sidebar ------------------------------------------------------------------------------------------

#[component]
fn SidebarPane() -> impl IntoView {
    use todo_client::sidebar_drag::{Row, Target};
    use wasm_bindgen::JsCast;
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
        let destination = task_drag::destination(&room);
        let reorder = sidebar_drag::row(&room);
        let drag_item = n.clone();
        let link_room = link_room(c, room.clone());
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
            (RoomId::Trash, _) => (icon(Icon::TrashTwo).into_any(), None),
            (_, Some(g)) => (glyph(g).into_any(), Some(n.count)),
            _ => (().into_any(), None),
        };
        let link = view! {
            <a
                id=origin.map(room_actions::Origin::dom_id)
                class="nav-item"
                class:menu-anchor=move || origin.is_some_and(|o| c.room_actions.anchored(o))
                class:sub=nested
                data-task-destination=destination.map(task_drag::destination_attr)
                class:task-drop-target=move || destination.is_some_and(|d| c.drop_target(task_drag::Target::Destination(d)))
                class:active=active.clone()
                class:contains-active=contains_active
                aria-current=move || active().then_some("page")
                href=href
                data-list-hue=n.hue
                aria-keyshortcuts=reorder.map(|_| "Alt+ArrowUp Alt+ArrowDown")
                draggable=reorder.map(|_| "false")
                on:pointerdown=move |ev: web_sys::PointerEvent| {
                    if reorder.is_some() && (ev.pointer_type() != "touch" || ev.target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                        .is_some_and(|el| el.closest(".sidebar-touch-grip").ok().flatten().is_some()))
                    {
                        c.start_sidebar_drag(drag_item.clone(), &ev);
                    }
                }
                on:click=move |ev: web_sys::MouseEvent| {
                    if c.sidebar_click_blocked.get_untracked() && ev.detail() > 0 {
                        c.sidebar_click_blocked.set(false);
                        ev.prevent_default();
                    } else {
                        link_room(ev);
                    }
                }
                on:contextmenu=move |ev| {
                    if c.sidebar_active() { ev.prevent_default(); return; }
                    if let Some(origin) = origin.filter(|_| !c.read_only()) {
                        ev.prevent_default();
                        c.room_actions.open_menu(c, origin);
                    }
                }
                on:keydown=move |ev| {
                    if let Some(row) = reorder.filter(|_| ev.alt_key() && !ev.ctrl_key() && !ev.shift_key()
                        && !ev.meta_key() && !ev.is_composing() && !c.read_only() && !c.sidebar_active())
                        && matches!(ev.key().as_str(), "ArrowUp" | "ArrowDown")
                    {
                        ev.prevent_default();
                        c.nudge_sidebar(row, ev.key() == "ArrowDown");
                    } else if let Some(origin) = origin.filter(|_| room_actions::context_key(&ev) && !c.read_only()) {
                        ev.prevent_default();
                        if !ev.repeat() { c.room_actions.open_menu(c, origin); }
                    }
                }
            >
                {if reorder.is_some() {
                    view! { <span class="sidebar-touch-grip" aria-hidden="true">{lead}</span> }.into_any()
                } else { lead }}
                <span class="nav-name">{n.name.clone()}</span>
                {count.map(|n| view! { <span class="count">{n.to_string()}</span> })}
                {(pip > 0).then(|| view! {
                    <span class="count nav-pip">{format!("{pip} new")}</span>
                })}
            </a>
        };
        if let Some(room_actions::Origin(subject, _)) = origin {
            view! {
                <div class="nav-entry" class:nav-collection=collection.is_some()
                    data-sidebar-row=reorder.map(sidebar_drag::attr)
                    class:sidebar-drag-source=move || reorder.is_some_and(|row| c.sidebar_source(row))
                    class:sidebar-into=move || collection.is_some_and(|id| c.sidebar_target(Target::Into(id)))
                    class:sidebar-before=move || reorder.is_some_and(|row| c.sidebar_target(Target::Before(row)))
                    class:sidebar-after=move || reorder.filter(|_| collection.is_none()).is_some_and(|row| c.sidebar_target(Target::After(row)))>
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
            <nav class="nav" id="room-navigation"
                on:pointerdown=move |_| c.sidebar_click_blocked.set(false)
                on:pointermove=move |ev| c.move_sidebar_drag(&ev)
                on:pointerleave=move |_| c.leave_sidebar_drag()
                on:pointerup=move |ev| c.end_sidebar_drag(&ev)
                on:pointercancel=move |_| { c.cancel_sidebar_drag(); }
                on:lostpointercapture=move |ev: web_sys::PointerEvent| {
                    // Touch starts with implicit capture on the icon before we transfer it to nav.
                    if ev.target() == ev.current_target() { c.cancel_sidebar_drag(); }
                }
                on:scroll=move |_| c.room_actions.position_menu()>
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
                            {item(model::NavItem {
                                room: RoomId::Trash, name: todo_client::trash::TITLE.into(), count: 0,
                                hue: "agent-activity", glyph: None,
                            }, false, Vec::new(), 0)}
                        </div>
                        {(!sb.groups.is_empty()).then(|| view! {
                            <div class="nav-collections">
                                {sb.groups.into_iter().filter_map(|g| {
                                    let RoomId::Collection(id) = g.head.room else { return None };
                                    let children = g.lists.iter().map(|n| n.room.clone()).collect();
                                    let folded = move || c.collapsed.with(|ids| ids.contains(&id));
                                    Some(view! {
                                        <div class="nav-section nav-group" class:folded=folded
                                            class:sidebar-after=move || c.sidebar_target(Target::After(Row::Collection(id)))>
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
            inert=move || c.detail_modal.get() || c.ai.modal.get()
        >
            <div class="brand">
                <img class="brand-mark" src="/app/brand/icon-small.svg" alt="" />
                <span>"To-do"</span>
            </div>
            <div class="side-body" on:scroll=move |_| c.room_actions.position_menu()>
                {navigation}
                {move || c.new_room.get().map(|kind| view! { <NewRoom kind/> })}
            </div>
            <Show when=move || c.drawer.get()>
                <div class="sidebar-toast"><ToastView in_drawer=true/></div>
            </Show>
            <div class="side-foot">
                <NewRoomMenu/>
                <room_actions::SyncLine/>
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
        let open = self.open.get_untracked();
        let key = ev.key();
        // Keep task shortcuts out of an open menu, without taking native button activation away.
        // Tab still reaches the window listener for the phone drawer's focus trap.
        if open && key != "Tab" {
            ev.stop_propagation();
        }
        if ev.alt_key() || ev.ctrl_key() || ev.meta_key() || ev.is_composing() {
            return;
        }
        if open && ev.repeat() && key == "Enter" {
            ev.prevent_default();
            return;
        }
        let last = self.items.len() - 1;
        let from = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| self.items.iter().position(|id| *id == el.id()));
        let next = match key.as_str() {
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
    let c = ctx();
    use todo_client::date_format::DateFormat;
    let csrf = backend::csrf();
    let menu = FooterMenu::new(
        "open-settings",
        if csrf.is_some() {
            &[
                "open-admin",
                "sync-now",
                "reminder-notification-setting",
                "open-ai-settings",
                "device-settings",
                "date-format-setting",
                "sign-out",
            ]
        } else {
            &[
                "open-admin",
                "sync-now",
                "reminder-notification-setting",
                "open-ai-settings",
                "device-settings",
                "date-format-setting",
            ]
        },
    );
    let dialog = NodeRef::<html::Dialog>::new();
    let format_error = RwSignal::new(false);
    let origin = window().location().origin().unwrap_or_default();
    let sync_unavailable = |s: &Sync| {
        s.connection == Connection::Syncing || s.read_only || s.signed_out || s.offline_demo
    };
    let field = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if menu.open.get() {
            c.reminder_alerts.refresh_permission();
        }
    });
    view! {
        <div class="footer-menu" on:focusout=move |ev| menu.focus_out(ev) on:keydown=move |ev| menu.key(ev)>
            <button type="button" id=menu.trigger class="icon-button" title="Settings" aria-label="Settings"
                aria-haspopup="menu" aria-controls="settings-menu" aria-expanded=move || menu.open.get().to_string()
                on:click=move |_| menu.toggle()>{icon(Icon::Settings)}</button>
            <Show when=move || menu.open.get()>
                <div class="footer-popup settings-menu" id="settings-menu" role="menu" aria-labelledby=menu.trigger>
                    <div class="settings-server" role="none">
                        <span>"Signed in to"</span>
                        <span class="settings-origin" title=origin.clone()>{origin.clone()}</span>
                    </div>
                    <a id="open-admin" role="menuitem" tabindex="-1" href=todo_client::ADMIN_PATH
                        target="_blank" rel="noopener" on:click=move |_| {
                            // Leave the link mounted until the browser performs its default action.
                            request_animation_frame(move || {
                                if menu.open.get_untracked() { menu.close(); }
                            });
                        }
                        on:keydown=move |ev| {
                            use wasm_bindgen::JsCast;
                            if ev.key() == " " && !ev.is_composing() {
                                ev.prevent_default();
                                if !ev.repeat() && let Some(link) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                                    link.click();
                                }
                            }
                        }
                    >{icon(Icon::ExternalLink)}"Open admin pages"</a>
                    <button type="button" id="sync-now" role="menuitem" tabindex="-1"
                        aria-disabled=move || c.sync.with(sync_unavailable).to_string()
                        on:click=move |_| {
                            if !sync_unavailable(&backend::status()) {
                                menu.close();
                                backend::nudge();
                            }
                        }
                    >{icon(Icon::RefreshCw)}{move || if c.sync.with(|s| s.connection == Connection::Syncing) { "Syncing…" } else { "Sync now" }}</button>
                    <reminder_alerts::Setting/>
                    <button type="button" id="open-ai-settings" role="menuitem" tabindex="-1" aria-haspopup="dialog"
                        on:click=move |_| { menu.close(); c.ai.settings.set(Some(menu.trigger)); }
                    >{icon(Icon::Sparkles)}"AI assistant…"</button>
                    <button type="button" id="device-settings" role="menuitem" tabindex="-1" aria-haspopup="dialog"
                        on:click=move |_| { menu.close(); c.device_settings_open.set(true); }
                    >{icon(Icon::Link)}"Devices…"</button>
                    <button type="button" id="date-format-setting" role="menuitem" tabindex="-1" aria-haspopup="dialog"
                        on:click=move |_| {
                            menu.close();
                            format_error.set(false);
                            if let Some(dialog) = dialog.get() { let _ = dialog.show_modal(); }
                        }>
                        {icon(Icon::Calendar)}"Date format"
                        <span class="date-format-example">{move || c.now.with(|n| n.dates().date(n.today()))}</span>
                    </button>
                    {csrf.clone().map(|token| view! {
                        <form method="post" action="/auth/logout" role="none" on:submit=move |_| {
                            // The token may have changed since the page loaded (see `backend::csrf`).
                            c.ai.sign_out();
                            if let (Some(i), Some(t)) = (field.get(), backend::csrf()) {
                                i.set_value(&t);
                            }
                        }>
                            <input type="hidden" name="csrf" value=token node_ref=field/>
                            <button type="submit" id="sign-out" role="menuitem" tabindex="-1">{icon(Icon::LogOut)}"Sign out"</button>
                        </form>
                    })}
                </div>
            </Show>
        </div>
        <dialog class="room-dialog date-format-dialog" aria-labelledby="date-format-label" node_ref=dialog
            on:close=move |_| { platform::focus(menu.trigger); }>
            <label id="date-format-label" for="date-format-choice">"Date format"</label>
            <select id="date-format-choice" autofocus
                on:change=move |ev| {
                    if let Some(format) = DateFormat::from_key(&event_target_value(&ev)) {
                        c.now.update(|now| now.date_format = format);
                        format_error.set(!platform::save_date_format(format));
                    }
                }>
                {DateFormat::ALL.map(|format| view! {
                    <option value=format.key() selected=move || c.now.with(|n| n.date_format == format)>
                        {move || format.example(c.now.with(Now::today))}
                    </option>
                })}
            </select>
            <Show when=move || format_error.get()><p role="status">"This browser couldn't save the date format."</p></Show>
            <form method="dialog"><button type="submit" class="button">"Done"</button></form>
        </dialog>
    }
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
            <span class="app-bar-sync">
                {move || {
                    let s = c.sync.get();
                    (s.connection != Connection::Online || s.pending > 0)
                        .then(|| sync_presentation(&s, c.synced_at.get()).0.full)
                }}
            </span>
            <Show when=move || !c.room.with(RoomId::is_history)>
                <button type="button" class="filter-trigger" id="open-filter" aria-label="Filter tasks"
                    aria-keyshortcuts="/ Control+K" title="Filter tasks (/ or Ctrl+K)"
                    aria-expanded=move || c.filter_open.get().to_string()
                    on:click=move |_| c.open_filter()
                >{icon(Icon::Search)}<span>"Filter"</span><kbd class="keycap">"Ctrl K"</kbd></button>
            </Show>
            <assistant::Trigger/>
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
    let drop = email_drop::Target::new(node);
    let width = RwSignal::new(0.0_f64);
    let reached_bottom = RwSignal::new(0u64);
    node.on_load(move |el| {
        width.set(f64::from(el.client_width()));
        platform::observe_width(&el, move |w| {
            width.set(w);
            drop.position();
        });
    });
    Effect::new(move |prev: Option<RoomId>| {
        let room = c.room.get();
        if prev.is_some_and(|old| old != room) {
            drop.clear();
            if let Some(el) = node.get_untracked() {
                el.set_scroll_top(0);
            }
        }
        room
    });
    view! {
        <div class="room-frame">
            <div class="room" node_ref=node class:task-room=move || !c.room.with(RoomId::is_history)
                on:dragover=email_drop::ignore_files on:drop=email_drop::ignore_files
                on:scroll=move |_| {
                    drop.position();
                    c.room_actions.position_menu();
                    c.task_actions.position();
                    c.status_menu.position();
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
                <Show when=move || !c.room.with(RoomId::is_history)>
                    <RoomPills/>
                    <div id="room-view-panel" class="room-view-body" class:board-active=move || c.on_board()
                        class:timeline-active=move || c.on_timeline()
                        role=move || (c.room_features.with(|f| f.workflow || f.timeline)).then_some("tabpanel")
                        aria-labelledby=move || c.room_features.with(|f| (f.workflow || f.timeline).then(|| workflow::view_id(c.room_view.get())))>
                        <Show when=move || c.on_timeline() fallback=move || view! {
                        <Show when=move || c.on_board() fallback=move || view! {
                            <QuickAdd/>
                            <div class="task-list-area" node_ref=drop.list
                                on:dragenter=move |ev| drop.over(c, ev, true)
                                on:dragover=move |ev| drop.over(c, ev, false)
                                on:dragleave=move |_| drop.leave()
                                on:dragend=move |_| drop.clear()
                                on:drop=move |ev| drop.receive(c, ev)>
                                <Show when=move || c.tomorrow.with(Option::is_some) fallback=|| view! { <TaskList/> }>
                                    <Handover/>
                                </Show>
                                <CompletedRows/>
                            </div>
                        }>
                            <board::Pane/>
                        </Show>
                        }>
                            <timeline::Pane/>
                        </Show>
                    </div>
                </Show>
                <Show when=move || c.room.with(|r| *r == RoomId::Trash)>
                    <trash::Trash/>
                </Show>
                <For each=move || c.room.with(|r| match r {
                    RoomId::Activity(agent) => vec![agent.clone()],
                    _ => Vec::new(),
                }) key=Clone::clone children=move |agent| view! { <activity::Feed agent=agent reached_bottom=reached_bottom/> }/>
            </div>
            <email_drop::Affordance target=drop c=c/>
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
    let views = move || c.room_features.with(|f| f.workflow || f.timeline);
    view! {
        <header class="room-head">
            <div class="head-text">
                <div class="eyebrow" class:has-views=views>
                    <EyebrowLine/>
                    <Show when=views>
                        <div class="eyebrow-actions"><workflow::Switcher/><room_actions::RoomMenuButton/></div>
                    </Show>
                </div>
                <div class="title-row">
                    <div class="room-title-group"><RoomTitle/><Show when=move || !views()><room_actions::RoomMenuButton/></Show></div>
                    <p class="summary">
                        {move || match c.view_room.with(|r| r.header.summary) {
                            // History rooms keep no score.
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
        let mut details: Vec<String> = e.details().into_iter().map(str::to_owned).collect();
        if c.on_timeline() && c.room.with(|r| matches!(r, RoomId::List(_))) {
            details.push(format!("{} scheduled", timeline::counts(c).0));
        }
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
    let count = Memo::new(move |_| {
        if c.on_timeline() {
            timeline::counts(c).2
        } else {
            c.view_room.with(|r| r.completed.len())
        }
    });
    let has_completed = move || !c.on_board() && count.get() > 0;
    view! {
        <div class="list-foot">
            <Show when=move || c.on_board()>
                <span class="board-foot-note caps">{icon(Icon::Columns3)}"Done shows last 7 days"</span>
            </Show>
            <Show when=has_completed>
                <button type="button" class="completed-toggle"
                    class:open=move || c.completed_open.get()
                    aria-expanded=move || c.completed_open.get().to_string()
                    aria-controls=move || if c.on_timeline() { "timeline-lanes" } else { "completed-rows" }
                    on:click=move |_| {
                        let open = !c.completed_open.get_untracked();
                        c.completed_open.set(open);
                        if open && !untrack(|| c.on_timeline()) {
                            request_animation_frame(|| {
                                if let Some(rows) = document().get_element_by_id("completed-rows") {
                                    rows.scroll_into_view();
                                }
                            });
                        }
                    }
                >
                    {icon(Icon::ChevronRight)}
                    <Show when=move || c.on_timeline() fallback=move || view! {
                        <span class="caps">"Completed"</span><span class="count">{move || count.get()}</span>
                    }>
                        <span class="caps">{move || format!("{} completed on Timeline ({})", if c.completed_open.get() { "Hide" } else { "Show" }, count.get())}</span>
                    </Show>
                </button>
            </Show>
            <Show when=move || c.on_timeline() && !c.read_only() && c.toast.with(Option::is_none)>
                <span class="timeline-foot-note">"Drag a bar edge to set Start or Due date"</span>
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
    due_chars: usize,
}

impl Columns {
    // Select a stylesheet slot rather than emitting an inline style under the server CSP.
    fn date_width(self) -> Option<&'static str> {
        match self.due_chars {
            0..=13 => None,
            14..=16 => Some("16"),
            17..=20 => Some("20"),
            21..=24 => Some("24"),
            _ => Some("32"),
        }
    }

    fn for_rows<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Self {
        let mut columns = Self::default();
        columns.include(rows);
        columns
    }

    fn include<'a>(&mut self, rows: impl IntoIterator<Item = &'a Row>) {
        for row in rows {
            self.agent |= row.agent.is_some();
            self.priority |= row.priority > 0;
            if let Some(due) = &row.due {
                self.due = true;
                if due.tone != DueTone::Overdue {
                    self.due_chars = self.due_chars.max(due.text.chars().count());
                }
            }
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
        if phase.is_none() && !c.modified_task_click(id, &ev) {
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
                    {d.text}
                </span>
            }
        })
    };
    let full_tags = row
        .tags
        .iter()
        .map(|t| t.label.as_str())
        .collect::<Vec<_>>()
        .join("  ");
    let tags = row.tags.clone().into_iter().enumerate().map(|(i, t)| view! {
        {(i > 0).then_some("  ")}<span class="tag-label" data-list-hue=t.hue>{t.label}</span>
    }).collect_view();
    let label = format!(
        "{} “{}”",
        if row.done { "Reopen" } else { "Complete" },
        row.title
    );
    view! {
        <li
            class="row"
            class:ai-updated=move || c.ai.highlights.with(|rows| rows.contains_key(&id))
            class:done=row.done
            class:task-dragging=move || c.dragging_row(id)
            class:task-drop-before=move || c.drop_target(task_drag::Target::Slot(todo_client::task_drag::Slot { row: id, after: false }))
            class:task-drop-after=move || c.drop_target(task_drag::Target::Slot(todo_client::task_drag::Slot { row: id, after: true }))
            class:completing=phase == Some(Phase::Hold)
            class:exiting=Phase::exiting(phase)
            inert=phase == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
            class:overdue=overdue
            class:selected=move || c.row_selected(id)
            class:bulk-selected=move || c.multi.with(|s| s.contains(id))
            class:bulk-cursor=move || c.multi.with(|s| s.cursor() == Some(id))
            class:with-agent=move || c.columns.with(|cols| cols.agent)
            class:with-priority=move || c.columns.with(|cols| cols.priority)
            class:with-due=move || c.columns.with(|cols| cols.due)
            data-date-width=move || c.columns.get().date_width()
            on:click=move |ev| c.click_task(id, &ev)
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
                    aria-current=move || (c.selected.get() == Some(id)).then_some("true")
                    aria-pressed=move || c.multi_active().then(|| c.multi.with(|s| s.contains(id).to_string()))
                    >{row.title.clone()}</button>
                <span class="meta">
                    <span class="due-slot">{due_view("due")}</span>
                    {row.chip.clone().map(|ch| chip(ch.label, ch.hue))}
                    {phase.is_none().then(|| view! { <workflow::RowStatus id=id/> })}
                    {row.steps.map(|(d, t)| view! { <span class="steps">{bars(d, t)}{format!("{d}/{t}")}</span> })}
                    {row.email.then(|| view! { <span class="meta-icon" title="From an email" aria-label="From an email" role="img">{icon(Icon::Mail)}</span> })}
                    {(!full_tags.is_empty()).then(|| view! { <span class="tag" title=full_tags>{tags}</span> })}
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
                    if !c.modified_task_click(id, &ev) {
                        c.submit_intent(|state| intent::toggle_important(state, id));
                    }
                }>
                {toggle_icon(Icon::Star, Icon::StarFilled)}
            </button>
            {phase.is_none().then(|| view! { <task_drag::Handle id=id title=row.title/> })}
            <span class="task-insertion-line" aria-hidden="true"></span>
        </li>
    }
}

#[component]
fn ToastView(#[prop(optional)] in_drawer: bool) -> impl IntoView {
    let c = ctx();
    let has_alert = Memo::new(move |_| {
        c.sync
            .with(|s| s.holding_upgrade || s.read_only || s.local_error.is_some())
    });
    let toast = Memo::new(move |_| {
        if has_alert.get() {
            return None;
        }
        let drawer = c.drawer.get();
        c.toast.with(|t| {
            t.as_ref()
                .filter(|t| (drawer && t.sidebar) == in_drawer)
                .cloned()
        })
    });
    let reminder = Memo::new(move |_| {
        if in_drawer || has_alert.get() || toast.with(Option::is_some) {
            return None;
        }
        let alarm = c.reminder_alerts.first()?;
        if !reminder_alerts::running(c) {
            return None;
        }
        c.state.with(|s| {
            let task = s.tasks.get(&alarm.task)?;
            alarm.is_current(s).then(|| (alarm, task.title.clone()))
        })
    });
    let dismiss = move || {
        if in_drawer {
            platform::focus("list-drawer");
        } else if c.detail_modal.get_untracked() {
            platform::focus("task-detail");
        }
        c.toast.set(None);
    };
    view! {
        // Keep the live region mounted before the toast arrives. Alerts announce themselves.
        <span class="toast-announcement" role="status" aria-live="polite" aria-atomic="true">
            {move || toast.with(|t| t.as_ref().map(|t| t.text.clone())).or_else(|| {
                reminder.with(|r| r.as_ref().map(|(_, title)| format!("Reminder: {title}")))
            })}
        </span>
        {move || {
            // Another tab waits to upgrade until this one's changes are stored.
            if !in_drawer && c.sync.with(|s| s.holding_upgrade) {
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
            if !in_drawer && c.read_only() {
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
            if !in_drawer && let Some(e) = c.sync.with(|s| s.local_error.clone()) {
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
            if let Some((alarm, title)) = reminder.get() {
                return Some(view! { <reminder_alerts::ReminderToast alarm title/> }.into_any());
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
                                                if in_drawer {
                                                    platform::focus("list-drawer");
                                                } else if c.detail_modal.get_untracked() {
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
            class:selected=move || c.row_selected(id)
            class:bulk-selected=move || c.multi.with(|s| s.contains(id))
            class:bulk-cursor=move || c.multi.with(|s| s.cursor() == Some(id))
            class:completing=move || phase.get() == Some(Phase::Hold)
            class:exiting=move || Phase::exiting(phase.get())
            inert=move || phase.get() == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
            on:click=move |ev| c.click_task(id, &ev)>
            <HandoverCheck row=row.clone() phase=phase/>
            <span class="sunrise">{icon(Icon::Sunrise)}</span>
            <div class="next-text">
                <button class="next-title" type="button" id=format!("row-{id}")
                    aria-pressed=move || c.multi_active().then(|| c.multi.with(|s| s.contains(id).to_string()))>{row.title}</button>
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
            class:selected=move || c.row_selected(id)
            class:bulk-selected=move || c.multi.with(|s| s.contains(id))
            class:bulk-cursor=move || c.multi.with(|s| s.cursor() == Some(id))
            class:completing=move || phase.get() == Some(Phase::Hold)
            class:exiting=move || Phase::exiting(phase.get())
            inert=move || phase.get() == Some(Phase::Depart)
            data-task-row=id.to_string()
            on:contextmenu=move |ev| task_actions::row_menu(c, id, &ev)
            on:click=move |ev| c.click_task(id, &ev)>
            <HandoverCheck row=row.clone() phase=phase/>
            <button class="t-title" type="button" id=format!("row-{id}")
                aria-pressed=move || c.multi_active().then(|| c.multi.with(|s| s.contains(id).to_string()))>{row.title}</button>
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
                if phase.with_untracked(Option::is_none) && !c.modified_task_click(row.id, &ev) {
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
            aria-modal=move || (c.detail_modal.get() && !c.ai.open.get()).then_some("true")
            inert=move || c.detail.with(Option::is_none) || c.ai.modal.get()
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
                    <workflow::DetailFields id=id/>
                    <DueEditor id=id detail=d/>
                    <PriorityEditor id=id detail=d/>
                    <repeat::RepeatEditor id=id detail=d/>
                    <tags::TagEditor task=id detail=d/>
                    <reminder::ReminderEditor id=id detail=d/>
                </dl>
            </section>
            <links::Links task=id detail=d/>
            <attachments::Section task=id detail=d uploads=uploads/>
            {move || d.with(|detail| detail.as_ref().map(|d| d.activity.clone())).map(|lines| view! { <DetailActivity lines=lines/> })}
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

const DUE_ANCHOR: &str = "edit-task-due";
const DUE_PICKER: &str = "detail-due-calendar";

#[component]
fn DueEditor(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let due = Memo::new(move |_| detail.with(|d| d.as_ref().and_then(|d| d.due.clone())));
    let popup = pickers::DetailPicker::new(c, id, DUE_ANCHOR, DUE_PICKER);
    let calendar = DatePicker::new(popup, "detail-active-day", "detail-month-label");
    let repeats = Signal::derive(move || {
        c.state
            .with(|s| s.tasks.get(&id).is_some_and(|t| t.recurrence.is_some()))
    });
    let start = move |_| {
        if c.read_only() {
            return;
        }
        let now = if c.live {
            platform::now()
        } else {
            c.now.get_untracked()
        };
        calendar.toggle(
            now.today(),
            due.with_untracked(|value| value.as_ref().map(|(day, _)| *day)),
        );
    };
    let choose = move |date| {
        if c.selected.get_untracked() == Some(id)
            && c.submit_intent(|s| intent::set_due(s, id, date))
        {
            popup.close(true);
        }
    };
    view! {
        <div class="prop" data-calendar-anchor="">
            <dt>{icon(Icon::Calendar)}"Due"</dt>
            <dd class="mono">
                <button id=DUE_ANCHOR type="button" class="prop-edit"
                    aria-label=move || format!("Edit due date: {}", due.with(|d| d.as_ref().map(|(_, label)| label.clone()).unwrap_or_else(|| "not set".into())))
                    title="Edit due date (D)" aria-keyshortcuts="D" disabled=move || c.read_only()
                    aria-haspopup="dialog" aria-controls=DUE_PICKER aria-expanded=move || popup.open.get().to_string()
                    on:click=start>
                    {move || due.with(|d| d.as_ref().map(|(_, label)| label.clone()).unwrap_or_else(|| "Set date".into()))}
                </button>
                <CalendarPopover c=c picker=calendar repeats=repeats choose=Callback::new(choose)/>
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
    let options = intent::quick_add::PRIORITIES;
    view! {
        <div class="prop">
            <dt>{icon(Icon::Flag)}<label for="task-priority">"Priority"</label></dt>
            <dd class="mono">
                <select id="task-priority" class="prop-input" title="Choose priority (Shift+P)" aria-keyshortcuts="Shift+P"
                    disabled=move || c.read_only()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        if let Some(option) = options.iter().find(|option| value == option.label) {
                            c.submit_intent(|s| intent::pick_priority(s, id, option.level));
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
                    {options.into_iter().rev().map(|option| {
                        let label = format!("{} {}", option.mark, option.label).trim().to_owned();
                        view! {
                            <option value=option.label prop:selected=move || priority.get() == option.level>{label}</option>
                        }
                    }).collect_view()}
                </select>
            </dd>
        </div>
    }
}

#[component]
fn DetailActivity(lines: Vec<todo_client::task_activity::Line>) -> impl IntoView {
    view! {
        <section class="d-section" aria-labelledby="task-activity-heading">
            <h3 class="caps d-caps" id="task-activity-heading">"Activity"</h3>
            <ul class="activity" role="list">
                {lines.into_iter().map(|line| {
                    let badge = match line.actor.as_ref() {
                        Some(actor @ Actor::Agent { .. }) => model::agent_mark(actor).map(|m| mark(&m)).into_any(),
                        Some(Actor::Owner { .. }) => you_mark().into_any(),
                        Some(Actor::EmailIngest) => view! { <span class="mark">{icon(Icon::Mail)}</span> }.into_any(),
                        _ => view! { <span class="mark">{icon(Icon::Clock)}</span> }.into_any(),
                    };
                    view! {
                        <li>
                            <span aria-hidden="true">{badge}</span>
                            <span class="act-text"><b>{line.who}</b>" "{line.action}</span>
                            <span class="act-when">{line.when}</span>
                        </li>
                    }
                }).collect_view()}
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
        <footer class="statusbar" inert=move || c.detail_modal.get() || c.ai.modal.get()>
            <span class="sb-counts">
                {move || c.room.with(|room| match room {
                    RoomId::Activity(agent) => Some(c.activity_feed.with(|f| {
                        f.counts(agent.as_ref(), &c.now.with(Now::local_time)).text()
                    })),
                    RoomId::Trash => Some(c.trash.with(|rows| todo_client::trash::status(rows, &c.now.with(Now::local_time)).text())),
                    _ if c.on_timeline() => {
                        let (scheduled, unscheduled, _) = timeline::counts(c);
                        Some(format!("{} open · {scheduled} scheduled · {unscheduled} unscheduled", scheduled + unscheduled))
                    }
                    _ => c.view_room.with(|r| r.header.status.map(|s| s.text())),
                })}
            </span>
            <span class="sb-keys" aria-hidden="true">
                <Show when=move || c.on_board()>
                    <span><kbd class="keycap">"H"</kbd><kbd class="keycap">"L"</kbd>" columns"</span>
                    <span><kbd class="keycap">"J"</kbd><kbd class="keycap">"K"</kbd>" cards"</span>
                    <Show when=move || !c.read_only()>
                        <span><kbd class="keycap">"Alt ←/→"</kbd>" move status"</span>
                        <span><kbd class="keycap">"S"</kbd>" status"</span>
                    </Show>
                </Show>
                <Show when=move || c.on_timeline()>
                    <span><kbd class="keycap">"J"</kbd><kbd class="keycap">"K"</kbd>" lanes"</span>
                    <span><kbd class="keycap">"0"</kbd>" today"</span>
                    <Show when=move || !c.read_only()>
                        <span><kbd class="keycap">"Alt ←/→"</kbd>" move dates"</span>
                        <span><kbd class="keycap">"[ / ]"</kbd>" duration"</span>
                    </Show>
                </Show>
                <Show when=move || c.room_features.with(|f| f.workflow || f.timeline)>
                    <span><kbd class="keycap">"V"</kbd>" view"</span>
                </Show>
                <Show when=move || !c.on_board() && !c.on_timeline() && (!c.shown.with(Vec::is_empty) || c.tomorrow.with(|t| t.as_ref().is_some_and(|t| t.rows().any(|r| !r.done))))>
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
                <Show when=move || !c.on_timeline() && !c.detail_modal.get() && !c.read_only() && c.view_room.with(|r| r.add_placeholder.is_some())>
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
