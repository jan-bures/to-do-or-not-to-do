//! Local task actions. The picker is a projection of the shared organizing intent; commands use
//! the same tracked Undo as completion. Like desktop and Android, task deletion does not ask.

use super::{
    Ctx, Linger, Phase, RoomId, Row, RowSection, ctx, glyph, icon, model, pickers, platform,
};
use leptos::html;
use leptos::prelude::*;
use std::time::Duration;
use todo_client::intent::{self, organize, organize::confirm::one_line};
use todo_core::State;
use todo_core::ids::{ListId, TaskId};
use todo_core::query::fold;
use todo_design::{Glyph, Icon, motion};
use wasm_bindgen::JsCast;

const MENU: &str = "task-action-menu";
const MOVE: &str = "task-move-picker";
const FILTER: &str = "task-move-filter";

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

    fn restore(self, c: Ctx) {
        let before = pickers::focused();
        let room = c.room.get_untracked();
        request_animation_frame(move || {
            if c.room.get_untracked() == room
                && !c.task_actions.open()
                && pickers::focus_unmoved(before.as_ref())
                && !platform::focus(&self.anchor())
                && !platform::focus("task-detail")
            {
                platform::focus("room-title");
            }
        });
    }
}

#[derive(Clone, Copy)]
pub(super) struct Controls {
    menu: RwSignal<Option<Origin>>,
    popover: NodeRef<html::Div>,
    moving: RwSignal<Option<Origin>>,
    dialog: NodeRef<html::Dialog>,
    filter: RwSignal<String>,
}

impl Controls {
    pub(super) fn new() -> Self {
        Self {
            menu: RwSignal::new(None),
            popover: NodeRef::new(),
            moving: RwSignal::new(None),
            dialog: NodeRef::new(),
            filter: RwSignal::new(String::new()),
        }
    }

    pub(super) fn open(self) -> bool {
        self.menu.with_untracked(Option::is_some) || self.moving.with_untracked(Option::is_some)
    }

    fn show_menu(self, c: Ctx, origin: Origin) {
        if self.menu.get_untracked() == Some(origin) {
            self.close();
            origin.restore(c);
            return;
        }
        if let Some(el) = self.popover.get_untracked() {
            // Give the popover a real return target, including after a pointer context click.
            platform::focus(&origin.anchor());
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

    fn show_move(self, c: Ctx, origin: Origin) {
        self.close();
        if let Some(dialog) = self.dialog.get_untracked() {
            self.filter.set(String::new());
            self.moving.set(Some(origin));
            if dialog.show_modal().is_err() {
                self.moving.set(None);
                origin.restore(c);
                return;
            }
            let before = pickers::focused();
            request_animation_frame(move || {
                if self.moving.get_untracked() == Some(origin)
                    && pickers::focus_unmoved(before.as_ref())
                {
                    platform::focus(FILTER);
                }
            });
        }
    }

    pub(super) fn position(self) {
        if let Some((origin, menu)) = self.menu.get_untracked().zip(self.popover.get_untracked())
            && !platform::place_room_menu(&menu, &origin.anchor())
        {
            self.close();
        }
    }

    pub(super) fn close(self) {
        if self.menu.try_update(Option::take).flatten().is_some()
            && let Some(el) = self.popover.get_untracked()
        {
            let _ = el.hide_popover();
        }
        if self.moving.try_update(Option::take).flatten().is_some() {
            super::room_actions::close_dialog(self.dialog);
        }
    }

    fn dismiss(self, c: Ctx) {
        let origin = self
            .menu
            .get_untracked()
            .or_else(|| self.moving.get_untracked());
        self.close();
        if let Some(origin) = origin {
            origin.restore(c);
        }
    }
}

fn task_available(state: &State, id: TaskId) -> bool {
    state
        .tasks
        .get(&id)
        .is_some_and(|t| t.merged_into.is_none() && state.is_live(id.into()))
}

pub(super) fn row_menu(c: Ctx, task: TaskId, ev: &leptos::ev::MouseEvent) {
    if c.read_only() || !c.state.with_untracked(|s| task_available(s, task)) {
        return;
    }
    ev.prevent_default();
    c.task_actions.show_menu(
        c,
        Origin {
            task,
            detail: false,
        },
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
    if !c.state.with_untracked(|s| task_available(s, origin.task)) {
        return false;
    }
    ev.prevent_default();
    if menu {
        c.task_actions.show_menu(c, origin);
    } else if moving {
        c.task_actions.show_move(c, origin);
    } else {
        change(c, origin, None);
    }
    true
}

/// Visible order, including the completed drawer (only when expanded). Departures are paint-only.
fn rows(c: Ctx) -> Vec<(RowSection, Row)> {
    let leaving = c.lingering.get_untracked();
    let departed = |r: &Row| {
        leaving
            .iter()
            .any(|l| l.phase == Phase::Depart && l.row.id == r.id)
    };
    let mut rows: Vec<_> = if let Some(plan) = c.tomorrow.get_untracked() {
        plan.rows()
            .filter(|r| !departed(r))
            .cloned()
            .map(|r| (RowSection::Handover, r))
            .collect()
    } else {
        c.shown.with_untracked(|shown| {
            shown
                .iter()
                .filter(|s| s.phase != Some(Phase::Depart))
                .map(|s| (RowSection::Open, s.row.clone()))
                .collect()
        })
    };
    if c.completed_open.get_untracked() {
        c.view_room.with_untracked(|room| {
            rows.extend(
                super::rows_with_exits(room, RowSection::Completed, &leaving)
                    .into_iter()
                    .filter(|s| s.phase.is_none())
                    .map(|s| (RowSection::Completed, s.row)),
            )
        });
    }
    rows
}

fn change(c: Ctx, origin: Origin, destination: Option<ListId>) {
    let task = origin.task;
    let cmd = c.state.with_untracked(|s| {
        if c.read_only() || !task_available(s, task) {
            return None;
        }
        let title = one_line(&s.tasks.get(&task)?.title);
        match destination {
            Some(list) => Some((
                intent::move_to_list(s, task, list)?,
                format!("Moved “{title}” to {}", one_line(&s.lists.get(&list)?.name)),
            )),
            None => Some((intent::delete_task(s, task)?, format!("Deleted “{title}”"))),
        }
    });
    c.task_actions.close();
    let Some((cmd, label)) = cmd else {
        return origin.restore(c);
    };
    let before = rows(c);
    if !c.submit_undoable(cmd, label, Some(task)) {
        return origin.restore(c);
    }
    c.lingering.update(|rows| rows.retain(|l| l.row.id != task));
    let after = rows(c);
    if after.iter().any(|(_, r)| r.id == task) {
        // A move in a smart room, for example, only changes the list chip and breadcrumb.
        return origin.restore(c);
    }
    let room = c.room.get_untracked();
    let mut next = None;
    if let Some(at) = before.iter().position(|(_, row)| row.id == task) {
        let (section, row) = &before[at];
        let usable = |(_, row): &&(RowSection, Row)| after.iter().any(|(_, r)| r.id == row.id);
        next = before[at + 1..]
            .iter()
            .chain(before[..at].iter().rev())
            .find(usable)
            .map(|(_, r)| r.id);
        if !platform::reduced_motion() {
            let token = c.toast_seq.get_untracked();
            let index = before[..at].iter().filter(|(s, _)| s == section).count();
            c.lingering.update(|rows| {
                rows.push(Linger {
                    token,
                    room: room.clone(),
                    section: *section,
                    row: row.clone(),
                    index,
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
            && !next.is_some_and(|id| platform::focus(&format!("row-{id}")))
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
            on:click=move |ev| { ev.prevent_default(); controls.show_menu(c, origin); }>
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
        if let Some(origin) = controls.menu.get().or_else(|| controls.moving.get()) {
            if c.read_only()
                || !c.state.with(|s| task_available(s, origin.task))
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
                if controls.popover.get_untracked().is_some_and(|el| !el.matches(":popover-open").unwrap_or(false)) {
                    controls.menu.set(None);
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
                on:click=move |_| { if let Some(origin) = controls.menu.get_untracked() { controls.show_move(c, origin); } }>
                {icon(Icon::List)}"Move to…"
            </button>
            <button type="button" class="danger" data-picker-option="" role="menuitem"
                on:click=move |_| { if let Some(origin) = controls.menu.get_untracked() { change(c, origin, None); } }>
                {icon(Icon::TrashTwo)}"Delete task"
            </button>
        </div>
        <MovePicker/>
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Choice {
    id: ListId,
    name: String,
    hue: &'static str,
    glyph: Glyph,
    current: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Group {
    heading: Option<String>,
    mark: Option<(&'static str, Glyph)>,
    choices: Vec<Choice>,
}

#[component]
fn MovePicker() -> impl IntoView {
    let c = ctx();
    let controls = c.task_actions;
    let groups = Memo::new(move |_| {
        let Some(origin) = controls.moving.get() else {
            return Vec::new();
        };
        controls.filter.with(|filter| {
            c.state.with(|s| {
                organize::task_move_groups(s, origin.task, filter)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|group| Group {
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
    view! {
        <dialog id=MOVE class="room-dialog task-move-picker" node_ref=controls.dialog aria-labelledby="task-move-heading"
            on:cancel=move |ev: web_sys::Event| { ev.prevent_default(); controls.dismiss(c); }
            on:click=move |ev| { if super::room_actions::scrim_click(&ev) { controls.dismiss(c); } }
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
                        <For each=move || groups.get() key=Clone::clone let:group>
                            <MoveGroup group=group/>
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
fn MoveGroup(group: Group) -> impl IntoView {
    let c = ctx();
    let name = group.heading.clone().unwrap_or_else(|| "Lists".into());
    view! {
        <div role="group" aria-label=name>
            {group.heading.map(|name| view! {
                <h3 class="caps task-move-group" aria-hidden="true">
                    {group.mark.map(|(hue, mark)| view! { <span class="move-glyph" data-list-hue=hue>{glyph(mark)}</span> })}
                    {name}
                </h3>
            })}
            {group.choices.into_iter().map(|choice| view! {
                <button type="button" data-picker-option="" class="move-choice" role="option"
                    disabled=choice.current aria-selected=choice.current.to_string()
                    on:click=move |_| {
                        if let Some(origin) = c.task_actions.moving.get_untracked() {
                            change(c, origin, Some(choice.id));
                        }
                    }>
                    <span class="move-glyph" data-list-hue=choice.hue>{glyph(choice.glyph)}</span>
                    <span class="picker-label">{choice.name}</span>
                    <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
                </button>
            }).collect_view()}
        </div>
    }
}
