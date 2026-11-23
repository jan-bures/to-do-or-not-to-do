//! Multi-selection for the task list; the shared client owns membership and action policy.

use super::{Ctx, Icon, Phase, TaskId, ctx, handover_row_phase, icon, model, platform};
use leptos::prelude::*;
use todo_client::selection::{self, BulkTaskAction, Selection, Tool};
use wasm_bindgen::JsCast;

impl Ctx {
    pub(super) fn bulk_visible(self) -> Vec<TaskId> {
        let mut ids: Vec<_> = if let Some(plan) = self.tomorrow.get() {
            self.lingering.with(|leaving| {
                plan.rows()
                    .filter(|r| handover_row_phase(leaving, r.id).is_none())
                    .map(|r| r.id)
                    .collect()
            })
        } else {
            self.shown.with(|rows| {
                rows.iter()
                    .filter(|s| s.phase.is_none())
                    .map(|s| s.row.id)
                    .collect()
            })
        };
        if self.completed_open.get() {
            self.view_room.with(|r| {
                self.lingering.with(|leaving| {
                    ids.extend(
                        r.completed
                            .iter()
                            .filter(|r| {
                                !leaving
                                    .iter()
                                    .any(|l| l.row.id == r.id && Phase::exiting(Some(l.phase)))
                            })
                            .map(|r| r.id),
                    );
                })
            });
        }
        self.state
            .with(|s| ids.retain(|id| s.is_live((*id).into())));
        ids
    }

    pub(super) fn bulk_ids(self) -> Vec<TaskId> {
        if !self.multi_active() {
            return Vec::new();
        }
        let visible = self.bulk_visible();
        self.multi.with(|s| s.ordered(&visible))
    }

    pub(super) fn multi_active(self) -> bool {
        self.multi.with(|s| s.cursor().is_some())
    }

    pub(super) fn row_selected(self, id: TaskId) -> bool {
        self.selected.get() == Some(id) || self.multi.with(|s| s.contains(id))
    }

    pub(super) fn click_task(self, id: TaskId, ev: &leptos::ev::MouseEvent) {
        if !self.modified_task_click(id, ev) {
            self.select(Some(id));
        }
    }

    pub(super) fn modified_task_click(self, id: TaskId, ev: &leptos::ev::MouseEvent) -> bool {
        if ev.shift_key() || ev.ctrl_key() || ev.meta_key() {
            ev.prevent_default();
            self.edit_multi(|s, visible, previous| {
                if ev.shift_key() {
                    s.range(visible, id, previous);
                } else {
                    s.toggle(visible, id, previous);
                }
            });
            true
        } else {
            false
        }
    }

    fn edit_multi(self, op: impl FnOnce(&mut Selection, &[TaskId], Option<TaskId>)) {
        let visible = untrack(|| self.bulk_visible());
        let previous = self.selected.get_untracked();
        self.multi.update(|s| op(s, &visible, previous));
        if previous.is_some() {
            // Selection replaces task detail, rather than opening a modal over the other rows.
            self.set_detail(None);
        }
        self.focus_bulk_row(self.multi.with_untracked(Selection::cursor).or(previous));
    }

    fn focus_bulk_row(self, cursor: Option<TaskId>) {
        request_animation_frame(move || {
            if !cursor.is_some_and(|id| platform::focus(&format!("row-{id}"))) {
                platform::focus("room-title");
            }
        });
    }

    fn clear_bulk(self) {
        let cursor = self.multi.with_untracked(Selection::cursor);
        self.multi.update(Selection::clear);
        self.focus_bulk_row(cursor);
    }

    pub(super) fn apply_bulk(self, action: BulkTaskAction) -> bool {
        let ids = untrack(|| self.bulk_ids());
        match self
            .state
            .with_untracked(|s| selection::batch(s, &ids, action))
        {
            Ok(Some((cmd, label))) => {
                if self.submit_undoable(cmd, label, None) {
                    self.clear_bulk();
                    true
                } else {
                    false
                }
            }
            Ok(None) => true,
            Err(message) => {
                self.show_toast(message.to_string(), false);
                false
            }
        }
    }

    pub(super) fn close_bulk_due(self, restore: bool) {
        if let Some(dialog) = self.bulk_due.get_untracked().filter(|d| d.open()) {
            dialog.close();
            if restore {
                platform::focus("bulk-due");
            }
        }
    }
}

pub(super) fn key(c: Ctx, ev: &leptos::ev::KeyboardEvent, key: &str) -> bool {
    let active = untrack(|| c.multi_active());
    if key == "escape" && active {
        ev.prevent_default();
        if !ev.repeat() {
            c.clear_bulk();
        }
        return true;
    }
    let list_focus = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_none_or(|el| {
            el.tag_name() == "BODY"
                || el
                    .closest("[data-task-row], #room-title, .bulk-bar, #task-detail")
                    .ok()
                    .flatten()
                    .is_some()
        });
    if !ev.alt_key() && list_focus {
        if (ev.ctrl_key() || ev.meta_key()) && !ev.shift_key() && key == "a" {
            ev.prevent_default();
            if !ev.repeat() {
                c.edit_multi(Selection::all);
            }
            return true;
        }
        if ev.shift_key()
            && !ev.ctrl_key()
            && !ev.meta_key()
            && matches!(key, "arrowup" | "arrowdown")
        {
            ev.prevent_default();
            c.edit_multi(|s, visible, previous| {
                s.extend(visible, if key == "arrowup" { -1 } else { 1 }, previous)
            });
            return true;
        }
    }
    if active
        && !ev.alt_key()
        && !ev.ctrl_key()
        && !ev.meta_key()
        && matches!(
            key,
            " " | "delete" | "m" | "d" | "p" | "r" | "t" | "l" | "f10"
        )
    {
        // Do not turn a multi-selection shortcut into a stray single-task edit. Native Space on
        // buttons, including toolbar controls, is left alone.
        if key != " " {
            ev.prevent_default();
        }
        return true;
    }
    false
}

fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Apply(BulkTaskAction::Completion { done: true }) => Icon::Check,
        Tool::Apply(BulkTaskAction::Completion { done: false }) => Icon::UndoTwo,
        Tool::Apply(BulkTaskAction::Important { .. }) => Icon::Star,
        Tool::Apply(BulkTaskAction::MyDay { .. }) => Icon::Sun,
        Tool::Move => Icon::List,
        Tool::Due => Icon::Calendar,
        Tool::Apply(BulkTaskAction::Delete) => Icon::TrashTwo,
        Tool::Apply(BulkTaskAction::Reorder { .. }) => Icon::GripVertical,
        Tool::Apply(_) => unreachable!("picker tools don't carry a value"),
    }
}

#[component]
pub(super) fn Bar() -> impl IntoView {
    let c = ctx();
    let ids = Memo::new(move |_| c.bulk_ids());
    let tools = Memo::new(move |_| {
        ids.with(|ids| {
            c.state
                .with(|s| selection::tools(s, ids, c.now.get().today()))
        })
    });
    view! {
        <Show when=move || ids.with(|ids| !ids.is_empty())>
            <section class="bulk-bar" aria-label="Task selection">
                <strong class="bulk-count" aria-live="polite">{move || ids.with(|ids| selection::count_label(ids.len()))}</strong>
                <div class="bulk-tools" role="group" aria-label="Bulk actions">
                    <For each=move || tools.get() key=|tool| format!("{tool:?}") let:tool>
                        <button type="button" class="bulk-tool" class:danger=matches!(tool, Tool::Apply(BulkTaskAction::Delete))
                            id=match tool { Tool::Move => Some("bulk-move"), Tool::Due => Some("bulk-due"), _ => None }
                            aria-label=tool.label() title=tool.label()
                            aria-haspopup=matches!(tool, Tool::Move | Tool::Due).then_some("dialog")
                            disabled=move || c.read_only()
                            on:click=move |_| match tool {
                                Tool::Apply(action) => { c.apply_bulk(action); }
                                Tool::Move => c.task_actions.show_bulk_move(),
                                Tool::Due => { if let Some(dialog) = c.bulk_due.get_untracked() { let _ = dialog.show_modal(); } }
                            }>
                            {icon(tool_icon(tool))}
                            <span aria-hidden="true">{tool.short_label()}</span>
                        </button>
                    </For>
                </div>
                <button type="button" class="icon-button bulk-close" aria-label="Clear selection" title="Clear selection (Esc)"
                    on:click=move |_| c.clear_bulk()>{icon(Icon::X)}</button>
            </section>
        </Show>
    }
}

#[component]
pub(super) fn DueDialog() -> impl IntoView {
    let c = ctx();
    let invalid = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let save = move |clear: bool| {
        let now = if c.live {
            platform::now()
        } else {
            c.now.get_untracked()
        };
        let date = if clear {
            Ok(None)
        } else {
            model::due_edit(&draft.get_untracked(), &now)
        };
        match date {
            Ok(date) => {
                if c.apply_bulk(BulkTaskAction::Due { date }) {
                    c.close_bulk_due(true);
                }
            }
            Err(_) => invalid.set(true),
        }
    };
    Effect::new(move |_| {
        if c.bulk_ids().len() < 2 || c.read_only() {
            c.close_bulk_due(false);
        }
    });
    view! {
        <dialog class="room-dialog" id="bulk-due-dialog" node_ref=c.bulk_due aria-labelledby="bulk-due-heading"
            on:close=move |_| { draft.set(String::new()); invalid.set(false); }
            on:cancel=move |ev: web_sys::Event| { ev.prevent_default(); c.close_bulk_due(true); }
            on:click=move |ev| { if super::room_actions::scrim_click(&ev) { c.close_bulk_due(true); } }
            on:keydown=move |ev| {
                ev.stop_propagation();
                super::room_actions::block_dialog_repeat(&ev);
                if ev.key() == "Tab" { platform::contain_tab("bulk-due-dialog", &ev); }
            }>
            <form on:submit=move |ev| { ev.prevent_default(); save(false); }>
                <h2 id="bulk-due-heading">"Due date for selected tasks"</h2>
                <input class="room-name-input" type="text" aria-label="Due date" autocomplete="off"
                    autofocus disabled=move || c.read_only() placeholder="tomorrow, fri, +3d…"
                    aria-invalid=move || invalid.get().to_string() aria-describedby="bulk-due-help"
                    prop:value=move || draft.get()
                    on:input=move |ev| { draft.set(event_target_value(&ev)); invalid.set(false); }/>
                <p id="bulk-due-help" class:prop-error=move || invalid.get()>
                    {move || if invalid.get() { "Try tomorrow, fri, +3d or a date like 2026-12-01." } else { "Clearing a date also clears its repeat rule." }}
                </p>
                <div class="dialog-actions">
                    <button type="button" class="button" on:click=move |_| c.close_bulk_due(true)>"Cancel"</button>
                    <button type="button" class="button" on:click=move |_| save(true)>"Clear due"</button>
                    <button type="submit" class="button primary" disabled=move || draft.with(|s| s.trim().is_empty())>"Save"</button>
                </div>
            </form>
        </dialog>
    }
}
