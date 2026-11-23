//! Web presentation of the shared workflow settings, stages and date intents.

use super::{Ctx, ListId, RoomId, TaskId, ctx, html, icon, model, pickers, platform, quick_add};
use leptos::prelude::*;
use todo_client::intent::workflow;
use todo_core::command::{BulkTaskAction, ListFeaturesPatch, Placement};
use todo_core::model::{ListFeatures, ListView, WorkflowStatus};
use todo_core::query::QueryStatus;
use wasm_bindgen::JsCast;

pub(super) const STAGES: [QueryStatus; 4] = [
    QueryStatus::Backlog,
    QueryStatus::Todo,
    QueryStatus::InProgress,
    QueryStatus::Done,
];
const VIEWS: [ListView; 3] = [ListView::List, ListView::Board, ListView::Timeline];
const PANEL: &str = "task-status-menu";
pub(super) const STARTS: &str = "edit-task-start";
const DETAIL_STATUS: &str = "edit-task-status";

pub(super) fn stage_info(stage: QueryStatus) -> (&'static str, &'static str, super::Icon) {
    use super::Icon::*;
    match stage {
        QueryStatus::Backlog => ("Backlog", "backlog", CircleDashed),
        QueryStatus::Todo => ("To do", "todo", Circle),
        QueryStatus::InProgress => ("In progress", "in-progress", CircleProgress),
        QueryStatus::Done => ("Done", "done", StatusDone),
    }
}

pub(super) fn open_stage(stage: QueryStatus) -> Option<WorkflowStatus> {
    match stage {
        QueryStatus::Backlog => Some(WorkflowStatus::Backlog),
        QueryStatus::Todo => Some(WorkflowStatus::Todo),
        QueryStatus::InProgress => Some(WorkflowStatus::InProgress),
        QueryStatus::Done => None,
    }
}

pub(super) fn stage(value: WorkflowStatus) -> QueryStatus {
    match value {
        WorkflowStatus::Backlog => QueryStatus::Backlog,
        WorkflowStatus::Todo => QueryStatus::Todo,
        WorkflowStatus::InProgress => QueryStatus::InProgress,
    }
}

fn view_info(view: ListView) -> (&'static str, super::Icon) {
    use super::Icon::*;
    match view {
        ListView::List => ("List", List),
        ListView::Board => ("Board", Columns3),
        ListView::Timeline => ("Timeline", GanttChart),
    }
}

fn available(features: ListFeatures, view: ListView) -> bool {
    features.allows(view) && view != ListView::Timeline
}

pub(super) fn effective(features: ListFeatures, override_view: Option<ListView>) -> ListView {
    override_view
        .filter(|&v| available(features, v))
        .unwrap_or_else(|| {
            if available(features, features.default_view) {
                features.default_view
            } else {
                ListView::List
            }
        })
}

pub(super) fn features(state: &todo_core::State, room: &RoomId) -> ListFeatures {
    match room {
        RoomId::List(id) if state.is_live((*id).into()) => state.lists[id].features,
        _ => ListFeatures::default(),
    }
}

impl Ctx {
    pub(super) fn on_board(self) -> bool {
        self.room_view.get() == ListView::Board
    }

    pub(super) fn switch_view(self, view: ListView) {
        if self.room_view.get_untracked() == view
            || !available(self.room_features.get_untracked(), view)
        {
            return;
        }
        self.cancel_task_drag();
        self.board_ui.reset_view();
        self.status_menu.close(false);
        self.room_actions.close_menu(false);
        self.multi.update(todo_client::selection::Selection::clear);
        self.view_override.set(Some(view));
        request_animation_frame(move || {
            if let Some(room) = document().query_selector(".room").ok().flatten() {
                room.set_scroll_top(0);
            }
            if !self.detail_modal.get_untracked() {
                platform::focus("room-title");
            }
        });
    }

    fn list_setting(self, id: ListId, patch: ListFeaturesPatch) {
        let setting_default = patch.default_view.is_some();
        let previous = self.room_view.get_untracked();
        if !self.submit_intent(|s| workflow::set_list_features(s, id, patch))
            || self.room.get_untracked() != RoomId::List(id)
        {
            return;
        }
        if setting_default {
            // A saved default applies on the next visit; do not move the room under the menu.
            self.view_override.set(Some(previous));
        } else if self
            .view_override
            .get_untracked()
            .is_some_and(|v| !available(self.room_features.get_untracked(), v))
        {
            self.view_override.set(None);
        }
    }

    pub(super) fn choose_status(self, id: TaskId, target: QueryStatus) {
        if self.read_only() {
            return;
        }
        let Some((old, list, title, stored)) = self
            .state
            .with_untracked(|s| {
                let task = s.tasks.get(&id)?;
                Some((
                    task_stage(s, id)?,
                    task.list,
                    task.title.clone(),
                    stage(task.status),
                ))
            })
            .filter(|(old, _, _, _)| *old != target)
        else {
            return;
        };
        if target == QueryStatus::Done {
            if let Some(row) = self
                .state
                .with_untracked(|s| model::board_row(s, id, self.now.get_untracked().today()))
            {
                self.complete(&row);
            }
        } else if let Some(cmd) = self.state.with_untracked(|s| {
            if old == QueryStatus::Done && target == stored {
                todo_client::intent::set_done(s, id, false)
            } else if old == QueryStatus::Done {
                todo_client::board::drop_command(s, list, id, target, Placement::Last)
            } else {
                workflow::set_status(s, id, open_stage(target)?)
            }
        }) {
            self.submit_undoable(
                cmd,
                format!("Moved “{title}” to {}", stage_info(target).0),
                None,
            );
        }
    }
}

#[component]
pub(super) fn Switcher() -> impl IntoView {
    let c = ctx();
    view! {
        <div class="view-switcher" role="tablist" aria-label="Room view"
            on:keydown=move |ev| {
                if matches!(ev.key().as_str(), "ArrowRight" | "ArrowLeft" | "Home" | "End") {
                    ev.prevent_default();
                    let view = match ev.key().as_str() {
                        "Home" => ListView::List,
                        "End" => ListView::Board,
                        _ if c.on_board() => ListView::List,
                        _ => ListView::Board,
                    };
                    if available(c.room_features.get_untracked(), view) {
                        c.switch_view(view);
                        request_animation_frame(move || { platform::focus(if view == ListView::Board { "view-board" } else { "view-list" }); });
                    }
                }
            }>
            {VIEWS.into_iter().map(|v| {
                let (label, glyph) = view_info(v);
                let id = match v { ListView::List => "view-list", ListView::Board => "view-board", ListView::Timeline => "view-timeline" };
                view! {
                    <button type="button" role="tab" id=id aria-label=label
                        aria-controls="room-view-panel"
                        hidden=move || !c.room_features.get().allows(v)
                        tabindex=move || if c.room_view.get() == v { "0" } else { "-1" }
                        aria-selected=move || (c.room_view.get() == v).to_string()
                        title=if v == ListView::Timeline { "Timeline view isn’t available on web yet" } else { label }
                        disabled=v == ListView::Timeline on:click=move |_| c.switch_view(v)>
                        {icon(glyph)}<span>{label}</span>
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
pub(super) fn Settings(id: ListId) -> impl IntoView {
    let c = ctx();
    let settings = Memo::new(move |_| features(&c.state.get(), &RoomId::List(id)));
    view! {
        <div class="list-settings-fields">
            {[("Workflow & Board", "Track Backlog, To do, In progress and Done, and unlock Board view.", true),
              ("Timeline & Start dates", "Add start dates to tasks and view them over time.", false)]
                .into_iter().map(|(label, description, workflow)| {
                    let is_on = move || if workflow { settings.get().workflow } else { settings.get().timeline };
                    let description_id = if workflow { "workflow-help" } else { "timeline-help" };
                    view! {
                        <div class="list-setting">
                            <button type="button" role="switch" aria-checked=move || is_on().to_string()
                                aria-describedby=description_id disabled=move || c.read_only()
                                on:click=move |_| {
                                    let f = settings.get_untracked();
                                    let mut patch = ListFeaturesPatch::default();
                                    if workflow { patch.workflow = Some(!f.workflow); } else { patch.timeline = Some(!f.timeline); }
                                    c.list_setting(id, patch);
                                }><span>{label}</span><i class="setting-switch" aria-hidden="true"></i></button>
                            <p id=description_id>{description}</p>
                        </div>
                    }
                }).collect_view()}
            <Show when=move || settings.with(|f| f.workflow || f.timeline)>
                <div class="list-setting">
                    <div class="list-setting-label" id="default-room-view">"Default view"</div>
                    <p>"Opens this view when you visit the list."</p>
                    <div class="view-switcher default-view" role="group" aria-labelledby="default-room-view">
                        {VIEWS.into_iter().map(|v| {
                            let (label, glyph) = view_info(v);
                            view! { <button type="button" hidden=move || !settings.get().allows(v)
                                title=if v == ListView::Timeline { "Timeline view isn’t available on web yet" } else { label }
                                aria-pressed=move || (settings.get().default_view == v).to_string()
                                disabled=move || v == ListView::Timeline || c.read_only()
                                on:click=move |_| c.list_setting(id, ListFeaturesPatch { default_view: Some(v), ..Default::default() })>
                                {icon(glyph)}<span>{label}</span>
                            </button> }
                        }).collect_view()}
                    </div>
                </div>
            </Show>
            <p class="list-setting-note">"Turning a feature off hides its view. Your task statuses and start dates are kept."</p>
        </div>
    }
}

pub(super) fn task_stage(s: &todo_core::State, id: TaskId) -> Option<QueryStatus> {
    let t = s.tasks.get(&id)?;
    (todo_client::intent::organize::is_actionable(s, t) && s.lists.get(&t.list)?.features.workflow)
        .then(|| QueryStatus::of(t))
}

pub(super) fn mark(stage: QueryStatus) -> impl IntoView {
    let (label, key, glyph) = stage_info(stage);
    view! { <span class="workflow-mark" data-stage=key>{icon(glyph)}<span>{label}</span></span> }
}

#[component]
pub(super) fn RowStatus(id: TaskId) -> impl IntoView {
    let c = ctx();
    let stage = Memo::new(move |_| {
        let own_list = c.room.with(|r| matches!(r, RoomId::List(_)));
        c.state.with(|s| task_stage(s, id)).filter(|stage| {
            own_list || matches!(stage, QueryStatus::Backlog | QueryStatus::InProgress)
        })
    });
    let anchor = StoredValue::new(format!("row-status-{id}"));
    view! {
        <Show when=move || stage.with(Option::is_some)>
            <button type="button" class="row-status" id=anchor.get_value() aria-haspopup="listbox" aria-controls=PANEL
                disabled=move || c.read_only() aria-label=move || stage.get().map(|v| format!("Status: {}", stage_info(v).0))
                aria-expanded=move || c.status_menu.active(&anchor.get_value())
                on:click=move |ev| { ev.stop_propagation(); c.status_menu.show(c, Some(id), anchor.get_value()); }>
                {move || stage.get().map(mark)}
            </button>
        </Show>
    }
}

#[component]
pub(super) fn DetailFields(id: TaskId) -> impl IntoView {
    let c = ctx();
    let stage = Memo::new(move |_| c.state.with(|s| task_stage(s, id)));
    let timeline = Memo::new(move |_| {
        c.state.with(|s| {
            s.tasks
                .get(&id)
                .and_then(|t| s.lists.get(&t.list))
                .is_some_and(|l| l.features.timeline)
        })
    });
    view! {
        <Show when=move || stage.with(Option::is_some)>
            <div class="prop">
                <dt>{icon(super::Icon::CircleProgress)}"Status"</dt>
                <dd>
                    <button type="button" class="prop-edit" id=DETAIL_STATUS title="Choose status (S)" aria-keyshortcuts="S"
                        aria-haspopup="listbox" aria-controls=PANEL aria-expanded=move || c.status_menu.active(DETAIL_STATUS)
                        disabled=move || c.read_only()
                        on:click=move |_| c.status_menu.show(c, Some(id), DETAIL_STATUS.into())>
                        {move || stage.get().map(mark)}
                    </button>
                </dd>
            </div>
        </Show>
        <Show when=move || timeline.get()><Starts id=id/></Show>
    }
}

#[component]
fn Starts(id: TaskId) -> impl IntoView {
    let c = ctx();
    let day = Memo::new(move |_| {
        c.state
            .with(|s| s.tasks.get(&id).and_then(|t| t.start_date))
    });
    let popup = pickers::DetailPicker::new(c, id, STARTS, "detail-start-calendar");
    let calendar = quick_add::DatePicker::new(popup, "start-active-day", "start-month-label");
    let choose = move |date| {
        if c.selected.get_untracked() == Some(id) && !c.read_only() {
            if let Some(cmd) = c.state.with_untracked(|s| workflow::set_start(s, id, date)) {
                let label = if date.is_some() {
                    "Set start date"
                } else {
                    "Cleared start date"
                };
                if !c.submit_undoable(cmd, label.into(), None) {
                    return;
                }
            }
            popup.close(true);
        }
    };
    view! {
        <div class="prop" data-calendar-anchor="">
            <dt>{icon(super::Icon::CalendarRange)}"Starts"</dt>
            <dd class="mono">
                <button type="button" id=STARTS class="prop-edit" title="Edit start date (Shift+D)" aria-keyshortcuts="Shift+D"
                    aria-label="Edit start date" aria-haspopup="dialog" aria-controls=popup.id
                    aria-expanded=move || popup.open.get().to_string() disabled=move || c.read_only()
                    on:click=move |_| calendar.toggle(if c.live { platform::now() } else { c.now.get_untracked() }.today(), day.get_untracked())>
                    {move || day.get().map_or_else(|| "Set date".into(), |d| model::short_date(d, c.now.get().today()))}
                </button>
                <quick_add::CalendarPopover c=c picker=calendar choose=Callback::new(choose) starts=Signal::derive(|| true)/>
            </dd>
        </div>
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Target {
    id: Option<TaskId>, // None is the current selection on Board.
    anchor: String,
}

#[derive(Clone, Copy)]
pub(super) struct StatusMenu {
    target: RwSignal<Option<Target>>,
    node: NodeRef<html::Div>,
}

impl StatusMenu {
    pub(super) fn new() -> Self {
        Self {
            target: RwSignal::new(None),
            node: NodeRef::new(),
        }
    }

    fn active(self, anchor: &str) -> String {
        self.target
            .with(|t| t.as_ref().is_some_and(|t| t.anchor == anchor))
            .to_string()
    }

    pub(super) fn show(self, c: Ctx, id: Option<TaskId>, anchor: String) {
        if c.read_only()
            || id.is_some_and(|id| c.state.with_untracked(|s| task_stage(s, id).is_none()))
        {
            return;
        }
        let Some(node) = self.node.get_untracked() else {
            return;
        };
        if self
            .target
            .with_untracked(|t| t.as_ref().is_some_and(|t| t.anchor == anchor))
        {
            self.close(true);
            return;
        }
        self.target.set(Some(Target { id, anchor }));
        if node.show_popover().is_ok() {
            request_animation_frame(move || self.position());
            pickers::focus_choice(PANEL, PANEL);
        } else {
            self.target.set(None);
        }
    }

    pub(super) fn position(self) {
        if let Some((target, node)) = self.target.get_untracked().zip(self.node.get_untracked())
            && !platform::place_room_menu(&node, &target.anchor)
        {
            self.close(false);
        }
    }

    pub(super) fn close(self, restore: bool) {
        if let Some(target) = self.target.try_update(Option::take).flatten() {
            if let Some(node) = self.node.get_untracked() {
                let _ = node.hide_popover();
            }
            if restore
                && !platform::focus(&target.anchor)
                && let Some(id) = target.id
            {
                platform::focus(&format!("row-{id}"));
            }
        }
    }
}

#[component]
pub(super) fn StatusOptions(
    current: Signal<Option<QueryStatus>>,
    choose: Callback<QueryStatus>,
    #[prop(optional)] open_only: bool,
) -> impl IntoView {
    STAGES.into_iter().filter(|s| !open_only || *s != QueryStatus::Done).map(|s| view! {
        <button type="button" role="option" tabindex="-1" data-picker-option="" class="room-action-item status-choice"
            aria-selected=move || (current.get() == Some(s)).to_string() on:click=move |_| choose.run(s)>
            {mark(s)}<span class="picker-tick" aria-hidden="true">{icon(super::Icon::Check)}</span>
        </button>
    }).collect_view()
}

#[component]
pub(super) fn StatusPopover() -> impl IntoView {
    let c = ctx();
    let menu = c.status_menu;
    let resize = window_event_listener(leptos::ev::resize, move |_| menu.position());
    on_cleanup(move || resize.remove());
    let current = Signal::derive(move || {
        menu.target
            .with(|t| t.as_ref().and_then(|t| t.id))
            .and_then(|id| c.state.with(|s| task_stage(s, id)))
    });
    Effect::new(move |_| {
        if let Some(target) = menu.target.get()
            && (c.read_only()
                || target.id.is_some_and(|_| current.get().is_none())
                || (target.id.is_none() && (!c.on_board() || !c.multi_active())))
        {
            menu.close(false);
        }
    });
    let choose = move |stage| {
        if let Some(target) = menu.target.get_untracked() {
            menu.close(true);
            if let Some(id) = target.id {
                c.choose_status(id, stage);
                if target.anchor != DETAIL_STATUS && untrack(|| c.on_board()) {
                    super::board::refocus(c, id);
                }
            } else if let Some(status) = open_stage(stage) {
                c.apply_bulk(BulkTaskAction::Status { status });
            } else {
                c.apply_bulk(BulkTaskAction::Completion { done: true });
            }
        }
    };
    view! {
        <div class="room-action-menu status-menu" id=PANEL tabindex="-1" autofocus popover="auto" node_ref=menu.node
            role="listbox" aria-label="Choose status" on:toggle=move |_| {
                if menu.node.get_untracked().is_some_and(|el| !el.matches(":popover-open").unwrap_or(false)) { menu.target.set(None); }
            }
            on:keydown=move |ev| {
                ev.stop_propagation();
                if ev.is_composing() { return; }
                match ev.key().as_str() {
                    "Escape" => { ev.prevent_default(); if !ev.repeat() { menu.close(true); } }
                    "z" if ev.ctrl_key() && !ev.alt_key() && !ev.shift_key() && !c.read_only() => {
                        ev.prevent_default();
                        if !ev.repeat() { menu.close(true); c.undo(); }
                    }
                    "Tab" => platform::contain_tab(PANEL, &ev),
                    "Enter" if ev.repeat() => ev.prevent_default(),
                    _ => pickers::choice_key(PANEL, 1, &ev),
                }
            } on:focusin=pickers::choice_focus>
            <StatusOptions current=current choose=Callback::new(choose)/>
        </div>
    }
}

pub(super) fn click_control(id: &str) -> bool {
    if platform::focus(id)
        && let Some(button) = document()
            .get_element_by_id(id)
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
    {
        button.click();
        return true;
    }
    false
}

pub(super) fn key(c: Ctx, ev: &leptos::ev::KeyboardEvent, key: &str, modal: bool) -> bool {
    if ev.alt_key() {
        return false;
    }
    let in_detail = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| el.closest("#task-detail").ok().flatten().is_some());
    if !ev.shift_key() && !modal && !in_detail && matches!(key, "v" | "1" | "2" | "3") {
        let features = c.room_features.get_untracked();
        if !features.workflow && !features.timeline {
            return false;
        }
        let view = match key {
            "1" => ListView::List,
            "2" => ListView::Board,
            "3" => ListView::Timeline,
            _ if untrack(|| c.on_board()) => ListView::List,
            _ => ListView::Board,
        };
        c.switch_view(view);
        return true;
    }
    if c.read_only() || untrack(|| c.multi_active()) {
        return false;
    }
    if key == "d" && ev.shift_key() && (!untrack(|| c.on_board()) || modal || in_detail) {
        return click_control(STARTS);
    }
    if key != "s" {
        return false;
    }
    let board = untrack(|| c.on_board());
    let id = if board && !modal && !in_detail {
        super::board::cursor_card(c)
    } else {
        c.selected.get_untracked()
    };
    let Some((id, stage)) = id.and_then(|id| {
        c.state
            .with_untracked(|s| task_stage(s, id))
            .map(|stage| (id, stage))
    }) else {
        return false;
    };
    if ev.shift_key() {
        c.choose_status(id, STAGES[(stage as usize + 1) % STAGES.len()]);
        if board && !modal && !in_detail {
            super::board::refocus(c, id);
        }
    } else {
        let anchor = if modal || in_detail {
            DETAIL_STATUS.into()
        } else if board {
            format!("row-{id}")
        } else if document()
            .get_element_by_id(&format!("row-status-{id}"))
            .is_some()
        {
            format!("row-status-{id}")
        } else {
            format!("row-{id}")
        };
        c.status_menu.show(c, Some(id), anchor);
    }
    true
}
