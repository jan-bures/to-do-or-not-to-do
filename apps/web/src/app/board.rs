//! Board layout and input. The shared board owns grouping, ordering and command placement.

use super::{
    Ctx, ListId, Now, Phase, Room, RoomId, Row, TaskId, bars, ctx, html, icon, mark, model,
    platform, quick_add::QuickAdd, tick_svg, toggle_icon, workflow,
};
use leptos::prelude::*;
use todo_client::board::{Board, drop_command, project};
use todo_core::{Command, State, command::Placement, query::QueryStatus};
use todo_design::{Icon, layout::space};
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, PointerEvent};

pub(super) fn projection(state: &State, room: &Room, now: Now) -> Option<Board> {
    let RoomId::List(list) = room.id else {
        return None;
    };
    let mut board = project(state, list, &now.eval())?;
    if room.filtered {
        let ids: std::collections::BTreeSet<_> = room
            .rows
            .iter()
            .chain(&room.completed)
            .map(|r| r.id)
            .collect();
        for col in &mut board.columns {
            col.cards.retain(|id| ids.contains(id));
        }
        board.total_done = room.completed.len();
    }
    Some(board)
}

#[derive(Clone, Copy)]
struct Cursor {
    column: usize,
    index: usize,
    card: Option<TaskId>,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            column: QueryStatus::Todo as usize,
            index: 0,
            card: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Ui {
    cursor: RwSignal<Cursor>,
    pub(super) composer: RwSignal<Option<QueryStatus>>,
    rail: RwSignal<Option<bool>>,
    drag: RwSignal<Option<Drag>>,
    suppress_click: StoredValue<bool>,
}

impl Ui {
    pub(super) fn new() -> Self {
        Self {
            cursor: RwSignal::new(Cursor::default()),
            composer: RwSignal::new(None),
            rail: RwSignal::new(None),
            drag: RwSignal::new(None),
            suppress_click: StoredValue::new(false),
        }
    }

    pub(super) fn reset_view(self) {
        self.cursor.set(Cursor::default());
        self.composer.set(None);
        self.cancel();
    }

    pub(super) fn load_rail(self, room: &RoomId) {
        self.rail.set(match room {
            RoomId::List(id) => platform::load_done_rail(*id),
            _ => None,
        });
    }

    pub(super) fn dragging(self) -> bool {
        self.drag.with(|g| g.as_ref().is_some_and(|g| g.active))
    }

    pub(super) fn cancel(self) -> bool {
        let active = self
            .drag
            .try_update(Option::take)
            .flatten()
            .is_some_and(|g| g.active);
        if active {
            self.suppress_click.set_value(true);
        }
        active
    }
}

fn rail(c: Ctx) -> bool {
    c.board_ui
        .rail
        .get()
        .unwrap_or_else(|| !c.narrow.get() && c.detail.with(Option::is_some))
}

fn toggle_rail(c: Ctx) {
    let collapsed = !untrack(|| rail(c));
    c.board_ui.rail.set(Some(collapsed));
    c.board_ui.cursor.set(Cursor {
        column: QueryStatus::Done as usize,
        ..Default::default()
    });
    if let RoomId::List(list) = c.room.get_untracked() {
        platform::save_done_rail(list, collapsed);
    }
    request_animation_frame(move || {
        platform::focus(column_id(QueryStatus::Done));
    });
}

pub(super) fn visible(c: Ctx) -> Vec<TaskId> {
    let rail = rail(c);
    c.board.with(|b| {
        b.iter()
            .flat_map(|b| &b.columns)
            .filter(|col| !rail || col.stage != QueryStatus::Done)
            .flat_map(|col| col.cards.iter().copied())
            .filter(|id| !holding(c, *id))
            .collect()
    })
}

fn holding(c: Ctx, id: TaskId) -> bool {
    c.lingering
        .with(|ls| ls.iter().any(|l| l.row.id == id && l.board_slot.is_some()))
}

pub(super) fn slot(c: Ctx, id: TaskId) -> Option<(QueryStatus, usize)> {
    c.board.with_untracked(|b| {
        b.as_ref()?.columns.iter().find_map(|col| {
            col.cards
                .iter()
                .position(|&card| card == id)
                .map(|index| (col.stage, index))
        })
    })
}

fn column_id(stage: QueryStatus) -> &'static str {
    match stage {
        QueryStatus::Backlog => "board-backlog",
        QueryStatus::Todo => "board-todo",
        QueryStatus::InProgress => "board-in-progress",
        QueryStatus::Done => "board-done",
    }
}

pub(super) fn add_id(stage: QueryStatus) -> &'static str {
    match stage {
        QueryStatus::Backlog => "add-backlog",
        QueryStatus::InProgress => "add-in-progress",
        QueryStatus::Todo | QueryStatus::Done => "add-todo",
    }
}

fn aim(c: Ctx, id: TaskId) {
    if let Some((stage, index)) = slot(c, id) {
        c.board_ui.cursor.set(Cursor {
            column: stage as usize,
            index,
            card: Some(id),
        });
    }
}

fn focus_card(id: TaskId) -> bool {
    if platform::focus(&format!("row-{id}")) {
        // The board and each well are independently scrollable.
        if let Some(el) = document().get_element_by_id(&format!("row-{id}")) {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            opts.set_inline(web_sys::ScrollLogicalPosition::Nearest);
            el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
        true
    } else {
        false
    }
}

pub(super) fn refocus(c: Ctx, id: TaskId) {
    let before = super::pickers::focused();
    request_animation_frame(move || {
        if !untrack(|| c.on_board())
            || c.detail_modal.get_untracked()
            || !super::pickers::focus_unmoved(before.as_ref())
        {
            return;
        }
        aim(c, id);
        if !focus_card(id)
            && let Some((stage, _)) = slot(c, id)
        {
            platform::focus(column_id(stage));
        }
    });
}

fn cursor(c: Ctx) -> (Cursor, Option<TaskId>) {
    let current = c.board_ui.cursor.get_untracked();
    let cursor = current
        .card
        .and_then(|id| {
            slot(c, id).map(|(stage, index)| Cursor {
                column: stage as usize,
                index,
                card: Some(id),
            })
        })
        .unwrap_or(current);
    let card = if cursor.column == QueryStatus::Done as usize && untrack(|| rail(c)) {
        None
    } else {
        c.board.with_untracked(|b| {
            let column = &b.as_ref()?.columns[cursor.column].cards;
            column
                .get(cursor.index.min(column.len().saturating_sub(1)))
                .copied()
                .filter(|id| !untrack(|| holding(c, *id)))
        })
    };
    (cursor, card)
}

pub(super) fn cursor_card(c: Ctx) -> Option<TaskId> {
    cursor(c).1
}

fn begin_add(c: Ctx, stage: QueryStatus) {
    if c.read_only() {
        return;
    }
    let stage = if stage == QueryStatus::Done {
        QueryStatus::Todo
    } else {
        stage
    };
    c.board_ui.composer.set(Some(stage));
    c.board_ui.cursor.set(Cursor {
        column: stage as usize,
        ..Default::default()
    });
    request_animation_frame(|| {
        platform::focus("qa-input");
    });
}

fn move_card(c: Ctx, list: ListId, id: TaskId, target: DropSlot) {
    if c.read_only() {
        return;
    }
    let Some((cmd, row, from)) = c.state.with_untracked(|s| {
        Some((
            drop_command(s, list, id, target.stage, target.placement())?,
            model::board_row(s, id, c.now.get_untracked().dates())?,
            QueryStatus::of(s.tasks.get(&id)?),
        ))
    }) else {
        return;
    };
    if matches!(cmd, Command::CompleteTask { .. }) {
        c.complete(&row);
        refocus(c, id);
        return;
    }
    let label = if from == target.stage {
        format!("Reordered “{}”", row.title)
    } else {
        format!(
            "Moved “{}” to {}",
            row.title,
            workflow::stage_info(target.stage).0
        )
    };
    let before = platform::row_positions();
    if c.submit_undoable(cmd, label, None) {
        refocus(c, id);
        request_animation_frame(move || platform::animate_reorder(before));
    }
}

pub(super) fn key(c: Ctx, ev: &leptos::ev::KeyboardEvent, key: &str) -> bool {
    let RoomId::List(list) = c.room.get_untracked() else {
        return false;
    };
    let canvas = ev
        .target()
        .and_then(|t| t.dyn_into::<Element>().ok())
        .is_none_or(|el| {
            el.tag_name() == "BODY"
                || el.id() == "room-title"
                || el.class_list().contains("board-column")
                || el.class_list().contains("board-title")
                || (el.class_list().contains("done-rail")
                    && el.get_attribute("aria-expanded").as_deref() == Some("false"))
        });
    if !canvas || ev.shift_key() && key != "d" {
        return false;
    }
    let (at, card) = cursor(c);
    let current = workflow::STAGES[at.column];
    let (horizontal, by) = match (key, ev.alt_key()) {
        ("h" | "arrowleft", false) => (true, -1),
        ("l" | "arrowright", false) => (true, 1),
        ("arrowleft", true) if ev.ctrl_key() => (true, -1),
        ("arrowright", true) if ev.ctrl_key() => (true, 1),
        ("k", false) | ("arrowup", _) => (false, -1),
        ("j", false) | ("arrowdown", _) => (false, 1),
        _ => (false, 0),
    };
    if by != 0 {
        let col = if horizontal {
            (at.column as isize + by).clamp(0, 3) as usize
        } else {
            at.column
        };
        let dest = workflow::STAGES[col];
        let cards = c.board.with_untracked(|b| {
            b.as_ref()
                .map(|b| b.columns[col].cards.clone())
                .unwrap_or_default()
        });
        let index = if horizontal {
            at.index
        } else {
            at.index.saturating_add_signed(by)
        };
        if ev.alt_key() {
            if let Some(id) = card.filter(|_| col != at.column || !horizontal) {
                if horizontal {
                    move_card(
                        c,
                        list,
                        id,
                        DropSlot {
                            stage: dest,
                            anchor: cards.get(index).map(|&id| (id, false)),
                        },
                    );
                } else if let Some(&anchor) =
                    at.index.checked_add_signed(by).and_then(|i| cards.get(i))
                {
                    move_card(
                        c,
                        list,
                        id,
                        DropSlot {
                            stage: dest,
                            anchor: Some((anchor, by > 0)),
                        },
                    );
                }
            }
        } else {
            let index = index.min(cards.len().saturating_sub(1));
            let card = cards
                .get(index)
                .copied()
                .filter(|_| dest != QueryStatus::Done || !untrack(|| rail(c)));
            c.board_ui.cursor.set(Cursor {
                column: col,
                index,
                card,
            });
            if !card.is_some_and(focus_card) {
                platform::focus(column_id(dest));
            }
        }
        return true;
    }
    if ev.alt_key() {
        return false;
    }
    match key {
        "enter" if current == QueryStatus::Done && untrack(|| rail(c)) => toggle_rail(c),
        "n" => begin_add(c, current),
        " " if !c.read_only() => {
            if let Some(stage) =
                card.and_then(|id| c.state.with_untracked(|s| workflow::task_stage(s, id)))
                && stage == QueryStatus::Done
            {
                if let Some(id) = card {
                    let status = c.state.with_untracked(|s| {
                        s.tasks.get(&id).map(|task| workflow::stage(task.status))
                    });
                    if let Some(status) = status {
                        c.choose_status(id, status);
                        refocus(c, id);
                    }
                }
            } else if let Some(row) = card.and_then(|id| {
                c.state
                    .with_untracked(|s| model::board_row(s, id, c.now.get_untracked().dates()))
            }) {
                c.toggle(&row);
            } else {
                return false;
            }
        }
        "enter" | "e" | "d" => {
            let Some(id) = card else {
                return false;
            };
            c.select(Some(id));
            if !c.read_only() {
                if key == "e" {
                    c.editing.set(true);
                }
                if key == "d" {
                    let control = if ev.shift_key() {
                        workflow::STARTS
                    } else {
                        super::DUE_ANCHOR
                    };
                    // Detail and its modal focus are installed by effects before this callback.
                    set_timeout(
                        move || {
                            workflow::click_control(control);
                        },
                        std::time::Duration::ZERO,
                    );
                }
            }
        }
        _ => return false,
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DropSlot {
    stage: QueryStatus,
    anchor: Option<(TaskId, bool)>,
}

impl DropSlot {
    fn placement(self) -> Placement {
        match self.anchor {
            Some((id, true)) => Placement::After(id.into()),
            Some((id, false)) => Placement::Before(id.into()),
            None => Placement::Last,
        }
    }
}

#[derive(Clone)]
struct Drag {
    list: ListId,
    id: TaskId,
    from: QueryStatus,
    pointer: i32,
    start: (i32, i32),
    at: (i32, i32),
    offset: (f64, f64),
    width: f64,
    row: Row,
    active: bool,
    target: Option<DropSlot>,
}

fn target(c: Ctx, g: &Drag) -> Option<DropSlot> {
    if c.read_only() || c.room.get_untracked() != RoomId::List(g.list) {
        return None;
    }
    let el = document().element_from_point(g.at.0 as f32, g.at.1 as f32)?;
    let col = el.closest("[data-board-column]").ok().flatten()?;
    let key = col.get_attribute("data-board-column")?;
    let stage = workflow::STAGES
        .into_iter()
        .find(|s| workflow::stage_info(*s).1 == key)?;
    let anchor = if stage == QueryStatus::Done {
        None
    } else if let Some(card) = el.closest("[data-board-card]").ok().flatten() {
        let id = card.get_attribute("data-board-card")?.parse().ok()?;
        if id == g.id {
            return None;
        }
        let rect = card.get_bounding_client_rect();
        Some((id, f64::from(g.at.1) >= rect.top() + rect.height() * 0.5))
    } else {
        None
    };
    let target = DropSlot { stage, anchor };
    c.state
        .with_untracked(|s| {
            (workflow::task_stage(s, g.id)? == g.from).then_some(())?;
            drop_command(s, g.list, g.id, stage, target.placement())
        })
        .map(|_| target)
}

fn pickup(c: Ctx, ev: &PointerEvent, row: Row) {
    if ev.button() != 0 || !ev.is_primary() {
        return;
    }
    c.board_ui.suppress_click.set_value(false);
    if ev.ctrl_key()
        || ev.shift_key()
        || ev.meta_key()
        || c.read_only()
        || row.done && untrack(|| rail(c))
    {
        return;
    }
    let Some(target) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
        return;
    };
    let grip = target.closest(".board-grip").ok().flatten().is_some();
    if !grip
        && (ev.pointer_type() == "touch"
            || target
                .closest("button:not(.board-title)")
                .ok()
                .flatten()
                .is_some())
    {
        return;
    }
    let RoomId::List(list) = c.room.get_untracked() else {
        return;
    };
    let Some((from, _)) = slot(c, row.id) else {
        return;
    };
    if untrack(|| holding(c, row.id)) {
        return;
    }
    let Some(el) = ev
        .current_target()
        .and_then(|t| t.dyn_into::<Element>().ok())
    else {
        return;
    };
    if el.set_pointer_capture(ev.pointer_id()).is_err() {
        return;
    }
    let (x, y) = (ev.client_x(), ev.client_y());
    let rect = el.get_bounding_client_rect();
    c.board_ui.drag.set(Some(Drag {
        list,
        id: row.id,
        from,
        pointer: ev.pointer_id(),
        start: (x, y),
        at: (x, y),
        offset: (f64::from(x) - rect.x(), f64::from(y) - rect.y()),
        width: rect.width(),
        row,
        active: false,
        target: None,
    }));
}

fn pointer_move(c: Ctx, ev: &PointerEvent) {
    let Some(mut g) = c
        .board_ui
        .drag
        .get_untracked()
        .filter(|g| g.pointer == ev.pointer_id())
    else {
        return;
    };
    g.at = (ev.client_x(), ev.client_y());
    if !g.active && (g.at.0 - g.start.0).abs().max((g.at.1 - g.start.1).abs()) < space::S1 as i32 {
        return;
    }
    let first = !g.active;
    g.active = true;
    g.target = target(c, &g);
    c.board_ui.drag.set(Some(g));
    if first {
        request_animation_frame(move || scroll_drag(c));
    }
}

fn scroll_drag(c: Ctx) {
    let Some(mut g) = c
        .board_ui
        .drag
        .try_get_untracked()
        .flatten()
        .filter(|g| g.active)
    else {
        return;
    };
    let edge = f64::from(space::S8);
    let step =
        |gap: f64| ((edge - gap).clamp(0.0, edge) / edge * f64::from(space::S3)).ceil() as i32;
    if let Some(at) = document().element_from_point(g.at.0 as f32, g.at.1 as f32) {
        if let Some(el) = at.closest(".board").ok().flatten() {
            let rect = el.get_bounding_client_rect();
            let x = f64::from(g.at.0);
            let delta = step(rect.right() - x) - step(x - rect.left());
            if delta != 0 {
                el.set_scroll_left(el.scroll_left() + delta);
            }
        }
        if let Some(el) = at.closest(".board-cards").ok().flatten() {
            let rect = el.get_bounding_client_rect();
            let y = f64::from(g.at.1);
            let delta = step(rect.bottom() - y) - step(y - rect.top());
            if delta != 0 {
                el.set_scroll_top(el.scroll_top() + delta);
            }
        }
    }
    let next = target(c, &g);
    if g.target != next {
        g.target = next;
        c.board_ui.drag.set(Some(g));
    }
    request_animation_frame(move || scroll_drag(c));
}

fn pointer_up(c: Ctx, ev: &PointerEvent) {
    if let Some(mut g) = c
        .board_ui
        .drag
        .get_untracked()
        .filter(|g| g.pointer == ev.pointer_id())
    {
        c.board_ui.cancel();
        g.at = (ev.client_x(), ev.client_y());
        if g.active
            && let Some(t) = target(c, &g)
        {
            move_card(c, g.list, g.id, t);
        }
    }
    if ev.is_primary() && c.board_ui.suppress_click.get_value() {
        // A captured pointer can still dispatch a click even after Escape or an invalid drop.
        set_timeout(
            move || c.board_ui.suppress_click.set_value(false),
            std::time::Duration::ZERO,
        );
    }
}

fn insert(c: Ctx, stage: QueryStatus, anchor: Option<(TaskId, bool)>) -> bool {
    c.board_ui.drag.with(|g| {
        g.as_ref()
            .is_some_and(|g| g.target == Some(DropSlot { stage, anchor }))
    })
}

#[component]
pub(super) fn Pane() -> impl IntoView {
    let c = ctx();
    // Seed the independent keyboard cursor from the detail already shown on entering Board.
    if let Some(id) = c.selected.get_untracked() {
        aim(c, id);
    }
    let blur = window_event_listener(leptos::ev::blur, move |_| {
        c.board_ui.cancel();
    });
    on_cleanup(move || blur.remove());
    Effect::new(move |_| {
        if c.read_only() || c.drawer.get() || c.detail_modal.get() {
            c.board_ui.cancel();
        }
    });
    view! {
        <div class="board" class:with-rail=move || rail(c)
            class:board-drag-active=move || c.board_ui.dragging() aria-label="Board columns"
            on:scroll=move |_| c.status_menu.position()>
            {workflow::STAGES.map(|stage| view! { <Well stage=stage/> })}
        </div>
        <Lift/>
    }
}

#[component]
fn Well(stage: QueryStatus) -> impl IntoView {
    let c = ctx();
    let done = stage == QueryStatus::Done;
    let (label, stage_key, glyph) = workflow::stage_info(stage);
    let id = column_id(stage);
    let collapsed = move || done && rail(c);
    let cards = Memo::new(move |_| {
        let list = c.room.get();
        let mut ids = c.board.with(|b| {
            b.as_ref()
                .map(|b| b.columns[stage as usize].cards.clone())
                .unwrap_or_default()
        });
        c.lingering.with(|ls| {
            ids.retain(|id| {
                !ls.iter()
                    .any(|l| l.room == list && l.row.id == *id && l.board_slot.is_some())
            });
            for l in ls.iter().filter(|l| l.room == list) {
                if let Some((origin, index)) = l.board_slot
                    && origin == stage
                {
                    ids.insert(index.min(ids.len()), l.row.id);
                }
            }
        });
        ids
    });
    let total_done = move || {
        let total = c.board.with(|b| b.as_ref().map_or(0, |b| b.total_done));
        if c.view_room.with(|r| r.filtered) {
            return total;
        } // Already excludes held cards.
        let room = c.room.get();
        total.saturating_sub(c.lingering.with(|ls| {
            ls.iter()
                .filter(|l| l.room == room && l.board_slot.is_some())
                .count()
        }))
    };
    let hint = match stage {
        QueryStatus::Backlog => "Drop future ideas here",
        QueryStatus::Todo => "Nothing to do yet",
        QueryStatus::InProgress => "Nothing in progress",
        QueryStatus::Done => "No recent completions",
    };
    let add_label = format!("Add to {label}");
    let active = move || c.board_ui.composer.get() == Some(stage);
    view! {
        <section class="board-column" class:collapsed=collapsed id=(!done).then_some(id) tabindex="-1"
            data-board-column=stage_key aria-label=move || format!("{label}, {} tasks", cards.with(Vec::len))
            class:board-target=move || insert(c, stage, None)>
            <Show when=collapsed fallback=move || view! {
                <div class="board-column-head">
                    <h2><span data-stage=stage_key>{icon(glyph)}</span>{label}</h2>
                    <span class="board-count">{move || if done { format!("{} recent", cards.with(Vec::len)) } else { cards.with(Vec::len).to_string() }}</span>
                    {done.then(|| view! { <button type="button" class="icon-button done-rail" id=id title="Collapse Done"
                        aria-label="Collapse Done" aria-expanded="true" on:click=move |_| toggle_rail(c)>{icon(Icon::ChevronRight)}</button> })}
                </div>
                <div class="board-cards" on:scroll=move |_| c.status_menu.position()>
                    <For each=move || cards.get() key=|id| *id let:id>
                        <Card id=id stage=stage/>
                    </For>
                    <Show when=move || stage == QueryStatus::Backlog || cards.with(Vec::is_empty)><p class="board-empty">{hint}</p></Show>
                    <span class="board-last-slot" class:show-slot=move || !done && insert(c, stage, None) aria-hidden="true"></span>
                </div>
                {(!done).then(|| view! {
                    <Show when=active fallback={let add_label = add_label.clone(); move || view! {
                        <button type="button" class="board-add" id=add_id(stage) disabled=move || c.read_only()
                            on:click=move |_| begin_add(c, stage)>{icon(Icon::Plus)}{add_label.clone()}</button>
                    }}><QuickAdd column=stage/></Show>
                })}
                <Show when=move || done && (total_done() > 0)>
                    <button type="button" class="board-completed" on:click=move |_| {
                        c.preview_task(None);
                        c.switch_view(todo_core::model::ListView::List);
                        c.completed_open.set(true);
                        request_animation_frame(|| { if let Some(el) = document().get_element_by_id("completed-rows") { el.scroll_into_view(); } });
                    }>{move || format!("View all {} in Completed →", total_done())}</button>
                </Show>
            }>
                <button type="button" class="done-rail" id=id aria-expanded="false"
                    aria-label=move || format!("Expand Done, {} completed tasks", total_done()) title="Expand Done"
                    on:click=move |_| toggle_rail(c)>
                    <span data-stage="done">{icon(glyph)}</span><span class="board-rail-label">"Done"</span>
                    <span class="board-count">{move || cards.with(Vec::len)}</span>
                </button>
            </Show>
        </section>
    }
}

fn card_row(c: Ctx, id: TaskId) -> Option<Row> {
    if let Some(row) = c.lingering.with(|ls| {
        ls.iter()
            .find(|l| l.row.id == id && l.board_slot.is_some())
            .map(|l| l.row.clone())
    }) {
        return Some(row);
    }
    c.state
        .with(|s| model::board_row(s, id, c.now.get().dates()))
}

#[component]
fn Card(id: TaskId, stage: QueryStatus) -> impl IntoView {
    let c = ctx();
    let row = Memo::new(move |_| card_row(c, id));
    let title =
        Memo::new(move |_| row.with(|r| r.as_ref().map(|r| r.title.clone()).unwrap_or_default()));
    let phase = Memo::new(move |_| {
        c.lingering.with(|ls| {
            ls.iter()
                .find(|l| l.row.id == id && l.board_slot.is_some())
                .map(|l| l.phase)
        })
    });
    let done = stage == QueryStatus::Done;
    let overdue = move || {
        row.with(|r| {
            r.as_ref()
                .and_then(|r| r.due.as_ref())
                .is_some_and(|d| d.tone == model::DueTone::Overdue)
        })
    };
    let important = move || row.with(|r| r.as_ref().is_some_and(|r| r.priority == 3));
    let completed = Memo::new(move |_| done.then(|| completed_label(c, id)).flatten());
    let toggle = move || {
        if phase.get_untracked().is_some() || c.read_only() {
            return;
        }
        if done {
            if let Some(status) = c
                .state
                .with_untracked(|s| s.tasks.get(&id).map(|t| workflow::stage(t.status)))
            {
                c.choose_status(id, status);
                refocus(c, id);
            }
        } else if let Some(row) = row.get_untracked() {
            c.toggle(&row);
        }
    };
    view! {
        <article class="board-card-slot" data-board-card=id.to_string() data-task-row=id.to_string()
            class:board-drop-before=move || insert(c, stage, Some((id, false)))
            class:board-drop-after=move || insert(c, stage, Some((id, true)))
            class:board-exit=move || phase.get() == Some(Phase::Exit)
            class:board-placeholder=move || c.board_ui.drag.with(|g| g.as_ref().is_some_and(|g| g.active && g.id == id))
            on:pointerdown=move |ev| { if let Some(row) = row.get_untracked() { pickup(c, &ev, row); } }
            on:pointermove=move |ev| pointer_move(c, &ev) on:pointerup=move |ev| pointer_up(c, &ev)
            on:pointercancel=move |_| { c.board_ui.cancel(); } on:lostpointercapture=move |_| { c.board_ui.cancel(); }
            on:contextmenu=move |ev| super::task_actions::row_menu(c, id, &ev)
            on:click=move |ev| {
                if c.board_ui.suppress_click.get_value() || phase.get_untracked().is_some() { return; }
                aim(c, id);
                c.click_task(id, &ev);
                if !c.detail_modal.get_untracked() && !ev.ctrl_key() && !ev.meta_key() && !ev.shift_key() {
                    focus_card(id);
                }
            }>
            <div class="board-card" class:done=move || done || phase.get().is_some() class:completing=move || phase.get() == Some(Phase::Hold)
                class:overdue=overdue class:selected=move || c.row_selected(id)
                class:bulk-selected=move || c.multi.with(|s| s.contains(id))>
                <div class="board-card-head">
                    <button type="button" class="check" role="checkbox" aria-checked=move || (done || phase.get().is_some()).to_string()
                        aria-label=move || format!("{} “{}”", if done { "Reopen" } else { "Complete" }, title.get())
                        disabled=move || c.read_only() || phase.get().is_some()
                        on:click=move |ev| { ev.stop_propagation(); if !c.modified_task_click(id, &ev) { toggle(); } }>{tick_svg()}</button>
                    <button type="button" class="board-title" id=format!("row-{id}") title=move || title.get()
                        aria-current=move || (c.selected.get() == Some(id)).then_some("true")
                        aria-pressed=move || c.multi_active().then(|| c.multi.with(|s| s.contains(id).to_string()))
                        on:focus=move |_| aim(c, id)>{move || title.get()}</button>
                    <button type="button" class="icon-button task-toggle important-toggle board-important"
                        aria-label=move || format!("Important: {}", title.get()) aria-pressed=move || important().to_string()
                        disabled=move || c.read_only() || phase.get().is_some()
                        on:click=move |ev| { ev.stop_propagation(); if !c.modified_task_click(id, &ev) { c.submit_intent(|s| todo_client::intent::toggle_important(s, id)); } }>
                        {toggle_icon(Icon::Star, Icon::StarFilled)}
                    </button>
                </div>
                {move || row.get().map(|r| metadata(r, completed.get()))}
                <span class="board-grip" title="Drag card" aria-hidden="true">{icon(Icon::GripVertical)}</span>
            </div>
        </article>
    }
}

fn completed_label(c: Ctx, id: TaskId) -> Option<String> {
    let at = c.state.with(|s| s.tasks.get(&id)?.completed_at)?;
    let now = c.now.get();
    let local = platform::civil_time(at, now, c.live);
    let day = local.date();
    let label = match (day - now.today()).get_days() {
        0 => "Today".into(),
        -1 => "Yesterday".into(),
        _ => now.dates().weekday(day),
    };
    Some(format!("{label}, {}", local.strftime("%H:%M")))
}

fn metadata(row: Row, completed: Option<String>) -> impl IntoView {
    let has_meta = completed.is_some()
        || row.due.is_some()
        || row.steps.is_some()
        || row.priority > 0
        || row.email
        || !row.tags.is_empty()
        || row.agent.is_some();
    let rest = row.tags.len().saturating_sub(2);
    has_meta.then(|| view! {
        <div class="board-meta">
            {completed.map(|label| view! { <span class="board-completed-date mono">{label}</span> })}
            {row.due.map(|d| { let overdue = d.tone == model::DueTone::Overdue; view! {
                <span class="due" data-tone=if overdue { "overdue" } else { "later" }>
                    {overdue.then(|| view! { <i class="diamond" aria-hidden="true"></i> })}{d.text}
                </span>
            }})}
            {row.steps.filter(|_| !row.done).map(|(d, t)| view! { <span class="steps">{bars(d, t)}{format!("{d}/{t}")}</span> })}
            {(!row.done && row.priority > 0).then(|| view! { <span class="board-priority">{"!".repeat(usize::from(row.priority))}</span> })}
            {row.email.then(|| view! { <span class="meta-icon" role="img" aria-label="From an email" title="From an email">{icon(Icon::Mail)}</span> })}
            {row.tags.into_iter().take(2).map(|tag| view! { <span class="tag board-tag">{format!("#{tag}")}</span> }).collect_view()}
            {(rest > 0).then(|| view! { <span class="tag">{format!("+{rest}")}</span> })}
            {row.agent.as_ref().map(|agent| view! { <span class="board-agent">{mark(agent)}</span> })}
        </div>
    })
}

#[component]
fn Lift() -> impl IntoView {
    let c = ctx();
    let node = NodeRef::<html::Div>::new();
    let row = Memo::new(move |_| {
        c.board_ui
            .drag
            .with(|g| g.as_ref().filter(|g| g.active).map(|g| g.row.clone()))
    });
    Effect::new(move |_| {
        if let Some(el) = node.get()
            && let Some(g) = c.board_ui.drag.get().filter(|g| g.active)
        {
            let style = HtmlElement::style(&el);
            let _ =
                style.set_property("--lift-x", &format!("{}px", f64::from(g.at.0) - g.offset.0));
            let _ =
                style.set_property("--lift-y", &format!("{}px", f64::from(g.at.1) - g.offset.1));
            let _ = style.set_property("--lift-width", &format!("{}px", g.width));
        }
    });
    view! {
        <div class="board-card board-lift" node_ref=node aria-hidden="true" hidden=move || !c.board_ui.dragging()>
            {move || row.get().map(|r| view! {
                <div class="board-card-head"><span class="check">{tick_svg()}</span><span class="board-title">{r.title.clone()}</span></div>
                {let completed = r.done.then(|| completed_label(c, r.id)).flatten(); metadata(r, completed)}
            })}
        </div>
    }
}
