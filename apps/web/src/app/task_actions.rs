//! Local task actions. The picker is a projection of the shared organizing intent; commands use
//! the same tracked Undo as completion. Like desktop and Android, task deletion does not ask.

use super::{
    Ctx, Linger, Phase, RoomId, Row, RowSection, ctx, glyph, icon, model, pickers, platform,
};
use leptos::html;
use leptos::prelude::*;
use std::time::Duration;
use todo_client::intent::{
    self, merge, organize, organize::actionable_task, organize::confirm::one_line,
};
use todo_core::Command;
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::query::fold;
use todo_design::{Glyph, Icon, motion};
use wasm_bindgen::JsCast;

const MENU: &str = "task-action-menu";
const MOVE: &str = "task-move-picker";
const FILTER: &str = "task-move-filter";
const MERGE: &str = "task-merge-picker";
const MERGE_FILTER: &str = "task-merge-filter";
const MERGE_CANCEL: &str = "task-merge-cancel";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Origin {
    task: TaskId,
    detail: bool,
}

impl Origin {
    fn anchor(self) -> String {
        let prefix = if self.detail { "task-actions" } else { "row" };
        format!("{prefix}-{}", self.task)
    }

    fn restore(self, c: Ctx, opener: Option<web_sys::HtmlElement>) {
        let before = pickers::focused();
        let room = c.room.get_untracked();
        request_animation_frame(move || {
            if c.room.get_untracked() == room
                && !c.task_actions.open()
                && pickers::focus_unmoved(before.as_ref())
                && !opener.is_some_and(|el| {
                    el.is_connected()
                        && el.focus().is_ok()
                        && pickers::focused().as_ref() == Some(el.as_ref())
                })
                && !platform::focus(&self.anchor())
                && !platform::focus("task-detail")
            {
                platform::focus("room-title");
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Move {
    origin: Origin,
    from: ListId,
}

#[derive(Clone, Copy)]
pub(super) struct Controls {
    opener: StoredValue<Option<web_sys::HtmlElement>, LocalStorage>,
    menu: RwSignal<Option<Origin>>,
    popover: NodeRef<html::Div>,
    moving: RwSignal<Option<Move>>,
    bulk_moving: RwSignal<bool>,
    dialog: NodeRef<html::Dialog>,
    filter: RwSignal<String>,
    merging: RwSignal<Option<Origin>>,
    merge_dialog: NodeRef<html::Dialog>,
}

impl Controls {
    pub(super) fn new() -> Self {
        Self {
            opener: StoredValue::new_local(None),
            menu: RwSignal::new(None),
            popover: NodeRef::new(),
            moving: RwSignal::new(None),
            bulk_moving: RwSignal::new(false),
            dialog: NodeRef::new(),
            filter: RwSignal::new(String::new()),
            merging: RwSignal::new(None),
            merge_dialog: NodeRef::new(),
        }
    }

    pub(super) fn open(self) -> bool {
        self.menu.with_untracked(Option::is_some)
            || self.moving.with_untracked(Option::is_some)
            || self.bulk_moving.get_untracked()
            || self.merging.with_untracked(Option::is_some)
    }

    fn show_menu(self, c: Ctx, origin: Origin, opener: Option<web_sys::HtmlElement>) {
        if self.menu.get_untracked() == Some(origin) {
            self.dismiss(c);
            return;
        }
        if let Some(el) = self.popover.get_untracked() {
            // Give the popover a real return target, including after a pointer context click.
            if opener.is_none() {
                platform::focus(&origin.anchor());
            }
            self.opener.set_value(opener);
            self.menu.set(Some(origin));
            if el.show_popover().is_err() {
                self.menu.set(None);
                return;
            }
            self.position();
            let before = pickers::focused();
            request_animation_frame(move || {
                if self.menu.get_untracked() == Some(origin)
                    && pickers::focus_unmoved(before.as_ref())
                {
                    platform::focus("task-action-move");
                }
            });
        }
    }

    fn show_move(self, c: Ctx, origin: Origin, opener: Option<web_sys::HtmlElement>) {
        let menu_opener = self.close();
        let opener = opener.or(menu_opener);
        let from = c
            .state
            .with_untracked(|s| actionable_task(s, origin.task).map(|t| t.list));
        if let Some((dialog, from)) = self.dialog.get_untracked().zip(from) {
            let moving = Move { origin, from };
            self.filter.set(String::new());
            self.opener.set_value(opener);
            self.moving.set(Some(moving));
            if dialog.show_modal().is_err() {
                origin.restore(c, self.close());
                return;
            }
            let before = pickers::focused();
            request_animation_frame(move || {
                if self.moving.get_untracked() == Some(moving)
                    && pickers::focus_unmoved(before.as_ref())
                {
                    platform::focus(FILTER);
                }
            });
        }
    }

    pub(super) fn show_bulk_move(self) {
        let _ = self.close();
        if let Some(dialog) = self.dialog.get_untracked() {
            self.filter.set(String::new());
            self.bulk_moving.set(true);
            if dialog.show_modal().is_ok() {
                platform::focus(FILTER);
            } else {
                self.bulk_moving.set(false);
            }
        }
    }

    fn show_merge(self, c: Ctx, origin: Origin) {
        let opener = self.close();
        self.opener.set_value(opener);
        self.merging.set(Some(origin));
        if !self
            .merge_dialog
            .get_untracked()
            .is_some_and(|el| el.show_modal().is_ok())
        {
            self.dismiss(c);
            return;
        }
        merge_focus(self, origin, MERGE_FILTER);
    }

    pub(super) fn position(self) {
        if let Some((origin, menu)) = self.menu.get_untracked().zip(self.popover.get_untracked())
            && !platform::place_room_menu(&menu, &origin.anchor())
        {
            let _ = self.close();
        }
    }

    pub(super) fn close(self) -> Option<web_sys::HtmlElement> {
        let opener = self.opener.try_update_value(Option::take).flatten();
        if self.menu.try_update(Option::take).flatten().is_some()
            && let Some(el) = self.popover.get_untracked()
        {
            let _ = el.hide_popover();
        }
        if self.moving.try_update(Option::take).flatten().is_some()
            || self.bulk_moving.try_update(std::mem::take).unwrap_or(false)
        {
            super::room_actions::close_dialog(self.dialog);
        }
        if self.merging.try_update(Option::take).flatten().is_some() {
            super::room_actions::close_dialog(self.merge_dialog);
        }
        opener
    }

    fn dismiss(self, c: Ctx) {
        let origin = self
            .menu
            .get_untracked()
            .or_else(|| self.moving.get_untracked().map(|m| m.origin))
            .or_else(|| self.merging.get_untracked());
        let bulk = self.bulk_moving.get_untracked();
        let opener = self.close();
        if bulk {
            platform::focus("bulk-move");
        } else if let Some(origin) = origin {
            origin.restore(c, opener);
        }
    }
}

pub(super) fn row_menu(c: Ctx, task: TaskId, ev: &leptos::ev::MouseEvent) {
    if c.read_only()
        || untrack(|| c.multi_active())
        || !c
            .state
            .with_untracked(|s| actionable_task(s, task).is_some())
    {
        return;
    }
    ev.prevent_default();
    c.task_actions.show_menu(
        c,
        Origin {
            task,
            detail: false,
        },
        None,
    );
}

fn row_task(el: &web_sys::Element) -> Option<TaskId> {
    el.closest("[data-task-row]:not([inert])")
        .ok()
        .flatten()?
        .get_attribute("data-task-row")?
        .parse()
        .ok()
}

pub(super) fn shortcut(c: Ctx, ev: &leptos::ev::KeyboardEvent) -> bool {
    let menu = super::room_actions::context_key(ev);
    let key = ev.key();
    let moving = key.eq_ignore_ascii_case("m");
    if !menu && (ev.shift_key() || (!moving && key != "Delete")) {
        return false;
    }
    let Some(target) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };
    let origin = if let Some(task) = row_task(&target) {
        Origin {
            task,
            detail: false,
        }
    } else if target.tag_name() == "BODY" || target.closest("#task-detail").ok().flatten().is_some()
    {
        let Some(task) = c.selected.get_untracked() else {
            return false;
        };
        Origin {
            task,
            detail: target.tag_name() != "BODY" || c.detail_modal.get_untracked(),
        }
    } else {
        return false;
    };
    if !c
        .state
        .with_untracked(|s| actionable_task(s, origin.task).is_some())
    {
        return false;
    }
    ev.prevent_default();
    let opener = (target.tag_name() != "BODY")
        .then(|| target.dyn_into::<web_sys::HtmlElement>().ok())
        .flatten();
    if menu {
        c.task_actions.show_menu(c, origin, opener);
    } else if moving {
        c.task_actions.show_move(c, origin, opener);
    } else {
        change(c, origin, None);
    }
    true
}

/// Painted order, including rows in flight for their animation slots, not for focus.
fn rows(c: Ctx) -> Vec<(RowSection, Row, Option<Phase>)> {
    let leaving = c.lingering.get_untracked();
    let mut rows: Vec<_> = if let Some(plan) = c.tomorrow.get_untracked() {
        plan.rows()
            .map(|r| {
                (
                    RowSection::Handover,
                    r.clone(),
                    super::handover_row_phase(&leaving, r.id),
                )
            })
            .collect()
    } else {
        c.shown.with_untracked(|shown| {
            shown
                .iter()
                .map(|s| (RowSection::Open, s.row.clone(), s.phase))
                .collect()
        })
    };
    if c.completed_open.get_untracked() {
        c.view_room.with_untracked(|room| {
            rows.extend(
                super::rows_with_exits(room, RowSection::Completed, &leaving)
                    .into_iter()
                    .map(|s| (RowSection::Completed, s.row, s.phase)),
            );
        });
    }
    rows
}

fn change(c: Ctx, origin: Origin, destination: Option<(ListId, ListId)>) {
    let task = origin.task;
    let cmd = c.state.with_untracked(|s| {
        if c.read_only() {
            return None;
        }
        let title = one_line(&actionable_task(s, task)?.title);
        match destination {
            Some((from, list)) => Some((
                organize::move_task(s, task, from, list)?,
                format!("Moved “{title}” to {}", one_line(&s.lists.get(&list)?.name)),
            )),
            None => Some((intent::delete_task(s, task)?, format!("Deleted “{title}”"))),
        }
    });
    if let Some((cmd, label)) = cmd {
        apply_change(c, origin, cmd, label);
    } else {
        origin.restore(c, c.task_actions.close());
    }
}

fn apply_change(c: Ctx, origin: Origin, cmd: Command, label: String) {
    let task = origin.task;
    let opener = c.task_actions.close();
    let before = rows(c);
    if !c.submit_undoable(cmd, label, Some(task)) {
        return origin.restore(c, opener);
    }
    c.lingering.update(|rows| rows.retain(|l| l.row.id != task));
    let after = rows(c);
    if after
        .iter()
        .any(|(_, r, phase)| r.id == task && phase.is_none())
    {
        // A move in a smart room, for example, only changes the list chip and breadcrumb.
        return origin.restore(c, opener);
    }
    let room = c.room.get_untracked();
    let mut next = None;
    if let Some(at) = before.iter().position(|(_, row, _)| row.id == task) {
        let (section, row, _) = &before[at];
        let surviving = |id| {
            after
                .iter()
                .any(|(_, row, phase)| row.id == id && phase.is_none())
        };
        next = before[at + 1..]
            .iter()
            .chain(before[..at].iter().rev())
            .find(|(_, r, phase)| phase.is_none() && surviving(r.id))
            .map(|(_, r, _)| r.id);
        if !platform::reduced_motion() {
            let token = c.toast_seq.get_untracked();
            let index = before[..at].iter().filter(|(s, _, _)| s == section).count();
            c.lingering.update(|rows| {
                rows.push(Linger {
                    token,
                    room: room.clone(),
                    section: *section,
                    row: row.clone(),
                    index,
                    board_slot: None,
                    completion_stamp: None,
                    phase: Phase::Depart,
                })
            });
            let handover = *section == RowSection::Handover;
            let exit_room = room.clone();
            set_timeout(
                move || {
                    // The first hand-over card and the finished day's heading can replace the
                    // focused element. Restore the same target if the transition remounts it.
                    let focused = pickers::focused().and_then(|el| {
                        if el.id() == "room-title" {
                            Some(None)
                        } else if handover {
                            row_task(&el).map(Some)
                        } else {
                            None
                        }
                    });
                    c.lingering.update(|rows| rows.retain(|l| l.token != token));
                    if let Some(target) = focused
                        && c.room.get_untracked() == exit_room
                    {
                        focus_after(c, exit_room, target);
                    }
                },
                Duration::from_millis(u64::from(motion::ROW_COLLAPSE)),
            );
        }
    }
    if c.selected.get_untracked() == Some(task) {
        // Docked detail can follow the selection. Close a modal detail so the next row is reachable.
        c.select(
            if c.detail_modal.get_untracked() || c.narrow.get_untracked() {
                None
            } else {
                next
            },
        );
    }
    focus_after(c, room, next);
}

fn focus_after(c: Ctx, room: RoomId, next: Option<TaskId>) {
    let before = pickers::focused();
    request_animation_frame(move || {
        if c.room.get_untracked() == room
            && !c.task_actions.open()
            && pickers::focus_unmoved(before.as_ref())
            && !next.is_some_and(|id| {
                rows(c)
                    .iter()
                    .any(|(_, row, phase)| row.id == id && phase.is_none())
                    && platform::focus(&format!("row-{id}"))
            })
            && !platform::focus("room-title")
        {
            platform::focus("task-detail");
        }
    });
}

#[component]
pub(super) fn DetailMenu(task: TaskId) -> impl IntoView {
    let c = ctx();
    let controls = c.task_actions;
    let origin = Origin { task, detail: true };
    view! {
        <button type="button" class="icon-button" id=origin.anchor() title="Task actions"
            aria-label="Task actions" aria-haspopup="menu" aria-controls=MENU popovertarget=MENU
            aria-expanded=move || (controls.menu.get() == Some(origin)).to_string()
            disabled=move || c.read_only()
            on:click=move |ev| {
                ev.prevent_default();
                let opener = ev.current_target().and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
                controls.show_menu(c, origin, opener);
            }>
            {icon(Icon::Ellipsis)}
        </button>
    }
}

#[component]
pub(super) fn Overlays() -> impl IntoView {
    let c = ctx();
    let controls = c.task_actions;
    let resize = window_event_listener(leptos::ev::resize, move |_| controls.position());
    on_cleanup(move || resize.remove());
    Effect::new(move |_| {
        let moving = controls.moving.get();
        if controls.bulk_moving.get() && (c.bulk_ids().len() < 2 || c.read_only() || c.drawer.get())
        {
            controls.dismiss(c);
        }
        if let Some(origin) = controls
            .menu
            .get()
            .or_else(|| moving.map(|m| m.origin))
            .or_else(|| controls.merging.get())
        {
            if c.read_only()
                || !c.state.with(|s| {
                    actionable_task(s, origin.task)
                        .is_some_and(|t| moving.is_none_or(|m| t.list == m.from))
                })
                || (origin.detail && c.selected.get() != Some(origin.task))
                || c.drawer.get()
            {
                controls.dismiss(c);
            } else if controls.menu.get().is_some() {
                request_animation_frame(move || controls.position());
            }
        }
    });
    view! {
        <div id=MENU class="room-action-menu task-action-menu" role="menu" aria-label="Task actions"
            popover="auto" node_ref=controls.popover
            on:toggle=move |_| {
                if controls.popover.get_untracked().is_some_and(|el| !el.matches(":popover-open").unwrap_or(false))
                    && controls.menu.try_update(Option::take).flatten().is_some()
                {
                    controls.opener.set_value(None);
                }
            }
            on:keydown=move |ev| {
                ev.stop_propagation();
                if ev.is_composing() { return; }
                match ev.key().as_str() {
                    "Escape" => { ev.prevent_default(); if !ev.repeat() { controls.dismiss(c); } }
                    "Tab" => platform::contain_tab(MENU, &ev),
                    "Enter" if ev.repeat() => ev.prevent_default(),
                    _ => pickers::choice_key(MENU, 1, &ev),
                }
            }>
            <button type="button" id="task-action-move" data-picker-option="" role="menuitem" aria-haspopup="dialog"
                on:click=move |_| { if let Some(origin) = controls.menu.get_untracked() { controls.show_move(c, origin, None); } }>
                {icon(Icon::List)}"Move to…"
            </button>
            <button type="button" data-picker-option="" role="menuitem" aria-haspopup="dialog"
                on:click=move |_| { if let Some(origin) = controls.menu.get_untracked() { controls.show_merge(c, origin); } }>
                {icon(Icon::ListPlus)}"Merge into…"
            </button>
            <button type="button" class="danger" data-picker-option="" role="menuitem"
                on:click=move |_| { if let Some(origin) = controls.menu.get_untracked() { change(c, origin, None); } }>
                {icon(Icon::TrashTwo)}"Delete task"
            </button>
        </div>
        <MovePicker/>
        <MergePicker/>
    }
}

fn merge_focus(controls: Controls, origin: Origin, id: &'static str) {
    request_animation_frame(move || {
        if controls.merging.get_untracked() == Some(origin) {
            platform::focus(id);
        }
    });
}

#[component]
fn MergePicker() -> impl IntoView {
    let c = ctx();
    let controls = c.task_actions;
    let filter = RwSignal::new(String::new());
    let asked = RwSignal::new(None::<(TaskId, merge::Preview)>);
    let warning = RwSignal::new(false);
    Effect::new(move |_| {
        if controls.merging.get().is_none() {
            filter.set(String::new());
            asked.set(None);
            warning.set(false);
        }
    });
    let choices = Memo::new(move |_| {
        controls.merging.get().map_or_else(Vec::new, |origin| {
            filter.with(|text| {
                c.state
                    .with(|s| merge::candidates(s, origin.task, text).unwrap_or_default())
            })
        })
    });
    let confirming = move || asked.with(Option::is_some);
    let confirm = move |_| {
        let Some(origin) = controls.merging.get_untracked() else {
            return;
        };
        let Some((target, shown)) = asked.get_untracked() else {
            return;
        };
        // Check the live backend, not a UI snapshot that may not have redrawn after sync.
        // The check and submission run in the same browser turn.
        let state = super::backend::view();
        if let Some(cmd) = merge::command(&state, origin.task, target, &shown.question) {
            apply_change(c, origin, cmd, shown.toast);
        } else {
            let current = merge::preview(&state, origin.task, target);
            let focus = if current.is_some() {
                MERGE_CANCEL
            } else {
                MERGE_FILTER
            };
            asked.set(current.map(|p| (target, p)));
            warning.set(true);
            merge_focus(controls, origin, focus);
        }
    };
    view! {
        <dialog id=MERGE class="room-dialog task-move-picker task-merge-picker" node_ref=controls.merge_dialog
            aria-labelledby="task-merge-heading"
            aria-describedby=move || confirming().then_some("task-merge-description")
            on:cancel=move |ev: web_sys::Event| { ev.prevent_default(); controls.dismiss(c); }
            on:click=move |ev| { if super::room_actions::scrim_click(&ev) { controls.dismiss(c); } }
            on:keydown=move |ev| {
                ev.stop_propagation();
                if ev.is_composing() { return; }
                super::room_actions::block_dialog_repeat(&ev);
                match ev.key().as_str() {
                    "Escape" => { ev.prevent_default(); if !ev.repeat() { controls.dismiss(c); } }
                    "Tab" => platform::contain_tab(MERGE, &ev),
                    _ if !confirming() => pickers::choice_key_from_filter(MERGE, &ev),
                    _ => {}
                }
            }>
            <h2 id="task-merge-heading">{move || if confirming() { "Merge tasks?" } else { "Merge into…" }}</h2>
            <Show when=move || !confirming() fallback=move || view! {
                <div class="task-move-choices picker-page">
                    <p id="task-merge-description">{move || asked.with(|a| a.as_ref().map(|(_, p)| p.question.clone()))}</p>
                </div>
            }>
                <input type="text" id=MERGE_FILTER autofocus class="room-name-input" autocomplete="off"
                    aria-label="Filter open tasks" placeholder="Filter open tasks" prop:value=move || filter.get()
                    on:input=move |ev| filter.set(event_target_value(&ev)) />
                <div class="task-move-choices" role="group" aria-label="Destination tasks">
                    <Show when=move || choices.with(Vec::is_empty)>
                        <p role="status">{move || if filter.with(|f| f.trim().is_empty()) { merge::EMPTY } else { merge::NO_MATCHES }}</p>
                    </Show>
                    <For each=move || choices.get() key=|choice| (choice.id, choice.title.clone(), choice.location.clone()) let:choice>
                        <button type="button" class="move-choice" data-picker-option=""
                            on:click=move |_| {
                                if let Some(origin) = controls.merging.get_untracked()
                                    && let Some(preview) = c.state.with_untracked(|s| merge::preview(s, origin.task, choice.id))
                                {
                                    asked.set(Some((choice.id, preview)));
                                    warning.set(false);
                                    merge_focus(controls, origin, MERGE_CANCEL);
                                }
                            }>
                            {icon(Icon::ChevronRight)}
                            <span class="merge-choice-text">
                                <span class="picker-label">{choice.title}</span>
                                <span class="merge-location">{choice.location}</span>
                            </span>
                        </button>
                    </For>
                </div>
            </Show>
            <Show when=move || warning.get()><p role="status">{merge::CHANGED}</p></Show>
            <div class="dialog-actions">
                <button id=MERGE_CANCEL type="button" class="button" on:click=move |_| controls.dismiss(c)>"Cancel"</button>
                <Show when=confirming>
                    <button type="button" class="button primary" on:click=confirm>"Merge tasks"</button>
                </Show>
            </div>
        </dialog>
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Choice {
    id: ListId,
    name: String,
    hue: &'static str,
    glyph: Glyph,
    current: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct Group {
    id: Option<CollectionId>,
    heading: Option<String>,
    mark: Option<(&'static str, Glyph)>,
    choices: Vec<Choice>,
}

#[component]
fn MovePicker() -> impl IntoView {
    let c = ctx();
    let controls = c.task_actions;
    let groups = Memo::new(move |_| {
        let moving = controls.moving.get();
        let bulk = controls.bulk_moving.get();
        if moving.is_none() && !bulk {
            return Vec::new();
        }
        let ids = if bulk { c.bulk_ids() } else { Vec::new() };
        controls.filter.with(|filter| {
            c.state.with(|s| {
                let groups = if let Some(moving) = moving {
                    organize::task_move_groups(s, moving.origin.task, filter).unwrap_or_default()
                } else {
                    organize::task_destination_groups(s, filter, |id| {
                        !todo_client::selection::can_move(s, &ids, id)
                    })
                };
                groups
                    .into_iter()
                    .map(|group| Group {
                        id: group.collection.map(|c| c.id),
                        heading: group.heading,
                        mark: group.collection.map(|col| {
                            (
                                model::swatch(col.color.as_deref(), col.id.as_uuid().as_bytes()),
                                Glyph::for_collection(&fold(&col.name)),
                            )
                        }),
                        choices: group
                            .lists
                            .into_iter()
                            .map(|dest| {
                                let list = dest.list;
                                Choice {
                                    id: list.id,
                                    name: one_line(&list.name),
                                    current: dest.current,
                                    hue: model::swatch(
                                        list.color.as_deref(),
                                        list.id.as_uuid().as_bytes(),
                                    ),
                                    glyph: Glyph::for_list(&fold(&list.name)),
                                }
                            })
                            .collect(),
                    })
                    .collect()
            })
        })
    });
    // Reordering can drop focus even when the same button is still connected. Remember the list
    // by its button ID, which also survives moving the list to a different group.
    let focused_option = StoredValue::new_local(None::<String>);
    Effect::new(move |_| {
        groups.track();
        if let Some(moving) = controls.moving.get()
            && let Some(option) = focused_option.get_value()
        {
            request_animation_frame(move || {
                if controls.moving.get_untracked() == Some(moving)
                    && focused_option.with_value(|id| id.as_ref() == Some(&option))
                    && pickers::focus_unmoved(None)
                {
                    if document().get_element_by_id(&option).is_some() {
                        platform::focus(&option);
                    } else {
                        platform::focus(FILTER);
                    }
                }
            });
        }
    });
    view! {
        <dialog id=MOVE class="room-dialog task-move-picker" node_ref=controls.dialog aria-labelledby="task-move-heading"
            on:cancel=move |ev: web_sys::Event| { ev.prevent_default(); controls.dismiss(c); }
            on:click=move |ev| { if super::room_actions::scrim_click(&ev) { controls.dismiss(c); } }
            on:focusin=move |ev| focused_option.set_value(ev.target()
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                .filter(|el| el.has_attribute("data-picker-option"))
                .map(|el| el.id()))
            on:keydown=move |ev| {
                ev.stop_propagation();
                if ev.is_composing() { return; }
                super::room_actions::block_dialog_repeat(&ev);
                match ev.key().as_str() {
                    "Escape" => { ev.prevent_default(); if !ev.repeat() { controls.dismiss(c); } }
                    "Tab" => platform::contain_tab(MOVE, &ev),
                    _ => pickers::choice_key_from_filter(MOVE, &ev),
                }
            }>
            <h2 id="task-move-heading">"Move to…"</h2>
            <input type="text" id=FILTER autofocus class="room-name-input" autocomplete="off"
                aria-label="Filter lists" placeholder="Filter lists" prop:value=move || controls.filter.get()
                on:input=move |ev| controls.filter.set(event_target_value(&ev))/>
            <div class="task-move-choices">
                <Show when=move || !groups.with(Vec::is_empty) fallback=move || view! {
                    <p role="status">{move || format!("No list matches “{}”.", one_line(controls.filter.get().trim()))}</p>
                }>
                    <div role="listbox" aria-label="Destination lists">
                        <For each=move || groups.with(|groups| groups.iter().map(|g| g.id).collect::<Vec<_>>())
                            key=|id| *id let:id>
                            <MoveGroup id=id groups=groups/>
                        </For>
                    </div>
                </Show>
            </div>
            <div class="dialog-actions">
                <button type="button" class="button" on:click=move |_| controls.dismiss(c)>"Cancel"</button>
            </div>
        </dialog>
    }
}

#[component]
fn MoveGroup(#[prop(into)] id: Option<CollectionId>, groups: Memo<Vec<Group>>) -> impl IntoView {
    let group =
        Memo::new(move |_| groups.with(|groups| groups.iter().find(|g| g.id == id).cloned()));
    let heading = Memo::new(move |_| group.with(|g| g.as_ref().and_then(|g| g.heading.clone())));
    let mark = Memo::new(move |_| group.with(|g| g.as_ref().and_then(|g| g.mark)));
    view! {
        <div role="group" aria-label=move || heading.get().unwrap_or_else(|| "Lists".into())>
            <Show when=move || heading.with(Option::is_some)>
                <h3 class="caps task-move-group" aria-hidden="true">
                    {move || mark.get().map(|(hue, mark)| view! { <span class="move-glyph" data-list-hue=hue>{glyph(mark)}</span> })}
                    {move || heading.get()}
                </h3>
            </Show>
            <For each=move || group.with(|g| g.iter().flat_map(|g| g.choices.iter().map(|choice| choice.id)).collect::<Vec<_>>())
                key=|id| *id let:id>
                <MoveChoice id=id group=group/>
            </For>
        </div>
    }
}

#[component]
fn MoveChoice(id: ListId, group: Memo<Option<Group>>) -> impl IntoView {
    let c = ctx();
    let choice = Memo::new(move |_| {
        group.with(|g| g.as_ref()?.choices.iter().find(|c| c.id == id).cloned())
    });
    let current = move || choice.with(|c| c.as_ref().is_some_and(|c| c.current));
    let mark = Memo::new(move |_| choice.with(|c| c.as_ref().map(|c| (c.hue, c.glyph))));
    view! {
        <button type="button" id=format!("task-move-option-{id}") data-picker-option="" class="move-choice" role="option"
            disabled=current aria-selected=move || current().to_string()
            on:click=move |_| {
                if let Some(moving) = c.task_actions.moving.get_untracked() {
                    change(c, moving.origin, Some((moving.from, id)));
                } else if c.task_actions.bulk_moving.get_untracked()
                    && c.apply_bulk(todo_client::selection::BulkTaskAction::Move { list: id })
                {
                    c.task_actions.dismiss(c);
                }
            }>
            {move || mark.get().map(|(hue, mark)| view! { <span class="move-glyph" data-list-hue=hue>{glyph(mark)}</span> })}
            <span class="picker-label">{move || choice.with(|c| c.as_ref().map(|c| c.name.clone()))}</span>
            <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
        </button>
    }
}
