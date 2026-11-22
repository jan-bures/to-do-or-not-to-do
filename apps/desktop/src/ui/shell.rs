//! The main window: titlebar, sidebar, the room pane under its sky header, the detail pane and the status
//! bar. It draws a `model::Shell`, derived from the store's state whenever the data or the window's own
//! state changes, and turns clicks and keys into commands for the store.

mod detail;
mod sidebar;

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AsyncApp, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, MouseButton, ScrollHandle, SharedString, Stateful, StrikethroughStyle,
    UniformListScrollHandle, WeakEntity, Window, WindowControlArea, actions, div, prelude::*, px,
    uniform_list,
};
use todo_client::Connection;
use todo_core::ids::{CollectionId, TaskId};

use super::input::{InputColors, InputEvent, TextInput};
use super::kit;
use super::landscape::{Blend, HeaderCache, header_landscape, sun_at};
use crate::fixtures::Moment;
use crate::model::layout::{self, DetailPane, Width};
use crate::model::motion::{
    DetailMotion, Keep, Motion, Pace, RowLook, Wake, anchored_scroll, lifts, rows_rising_into,
};
use crate::model::{
    self, Chips, Due, Eyebrow, RoomKey, Shell, TaskRow, Tomorrow, UiState, commands, fit, hints,
    quick_add,
};
use crate::platform::win;
use crate::store::Store;
use crate::theme::{
    self, Hue, Icon, Palette, TextStyled, Theme, component, desktop, elevation, radius, space, ty,
};
use detail::Editors;
use sidebar::{Inline, Menu};

actions!(
    shell,
    [
        FocusQuickAdd,
        FocusFilter,
        NextTask,
        PreviousTask,
        EditTask,
        EditDue,
        CyclePriority,
        MoveTaskUp,
        MoveTaskDown,
        Escape
    ]
);

/// Key bindings for the main window. The single-letter ones only apply while the task list has focus,
/// never while typing.
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    let list = Some("TaskList");
    vec![
        KeyBinding::new("ctrl-n", FocusQuickAdd, Some("Shell")),
        KeyBinding::new("ctrl-k", FocusFilter, Some("Shell")),
        KeyBinding::new("e", EditTask, list),
        KeyBinding::new("d", EditDue, list),
        KeyBinding::new("p", CyclePriority, list),
        KeyBinding::new("alt-up", MoveTaskUp, list),
        KeyBinding::new("alt-down", MoveTaskDown, list),
        KeyBinding::new("j", NextTask, list),
        KeyBinding::new("down", NextTask, list),
        KeyBinding::new("k", PreviousTask, list),
        KeyBinding::new("up", PreviousTask, list),
        KeyBinding::new("escape", Escape, list),
    ]
}

/// Colours for an input on the raised quick-add surface.
pub fn quick_add_colors(p: &Palette) -> InputColors {
    InputColors {
        text: p.ink,
        placeholder: p.ink_3,
        caret: p.accent_strong,
        selection: theme::with_alpha(p.accent, 0.28),
    }
}

/// What the shell asks of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// Sign out: forget the token and the cache, and show pairing.
    SignOut,
}

/// A line for the status bar.
#[derive(Clone, Debug, PartialEq)]
struct Notice {
    text: SharedString,
    error: bool,
    /// A folder the notice is about, opened in Explorer by its "Open folder" action.
    folder: Option<std::path::PathBuf>,
}

impl Notice {
    fn info(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            error: false,
            folder: None,
        }
    }

    fn error(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            error: true,
            folder: None,
        }
    }
}

#[derive(Debug)]
struct OutgoingRoom {
    title: SharedString,
    eyebrow: Eyebrow,
    body: OutgoingBody,
}

#[derive(Debug)]
enum OutgoingBody {
    Rows {
        rows: Vec<(TaskRow, bool)>,
        cut: f32,
        chips: Chips,
    },
    Empty {
        line: SharedString,
        hint: SharedString,
    },
    Handover {
        tomorrow: Box<Tomorrow>,
        top: gpui::Pixels,
    },
    Plan(SharedString),
}

#[derive(Clone, Copy)]
struct RowStyle {
    room: Option<RoomKey>, // None for an outgoing (paint-only) layer.
    selected: bool,
    chips: Chips,
    look: Option<RowLook>,
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct ShellView {
    store: Rc<Store>,
    ui: UiState,
    shell: Shell,
    theme: Theme,
    palette: Palette,
    painted_hue: Hue,
    landscape: HeaderCache,
    quick_add: Entity<TextInput>,
    list_focus: FocusHandle,
    rows: UniformListScrollHandle,
    /// The finished day's hand-over, which scrolls when it's taller than the pane.
    handover_scroll: ScrollHandle,
    handover_bar: kit::Scrollbar,
    rows_bar: kit::Scrollbar,
    sidebar_scroll: ScrollHandle,
    sidebar_bar: kit::Scrollbar,
    detail_scroll: ScrollHandle,
    detail_bar: kit::Scrollbar,
    caption_preview: kit::CaptionPreview,
    menu: Option<Menu>,
    inline: Option<Inline>,
    /// The filter typed into the title bar.
    filter: Entity<TextInput>,
    edit: Editors,
    /// The completion choreography and the sun's travel (`model::motion`), sampled at `clock`.
    motion: Motion,
    clock: Clock,
    /// The time this frame draws (`clock` when the render started), so every row samples the same moment.
    frame: Duration,
    /// How far each row is drawn up its slot while rows above it collapse; empty at rest.
    row_lifts: Vec<f32>,
    /// The pending one-shot timer that wakes the view when a holding row starts to leave.
    alarm: Option<Duration>,
    /// A message for the status bar (a command the domain refused, a failed download).
    notice: Option<Notice>,
    /// The window's width class, as of the last frame.
    width: Width,
    /// Paint-only views of the outgoing room and detail; never used to resolve a command.
    outgoing: Option<OutgoingRoom>,
    closing: Option<model::Detail>,
    /// Old child rows, drawn without hitboxes until their collection finishes closing.
    closing_groups: HashMap<CollectionId, Vec<model::ListNav>>,
    /// Whether panes animate. The screenshot fixtures draw them at rest.
    animate: bool,
}

impl EventEmitter<ShellEvent> for ShellView {}

impl ShellView {
    pub fn new(
        store: Rc<Store>,
        ui: UiState,
        theme: Theme,
        typed: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell = model::derive(&store.view(), &ui, &store.sync(), &store.now());
        let palette = theme::palette(theme, shell.room.hue);
        let placeholder = shell.room.quick_add_placeholder.clone();
        let quick_add = cx.new(|cx| {
            let mut input = TextInput::new(placeholder, quick_add_colors(&palette), cx);
            if let Some(t) = typed {
                input.set_text(t.to_string(), cx);
            }
            input
        });
        cx.subscribe_in(
            &quick_add,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => {
                    let text = input.read(cx).content.to_string();
                    if this.add_task(&text, this.ui.room, cx) {
                        input.update(cx, |i, cx| i.reset(window, cx));
                    }
                }
                InputEvent::Cancel => window.focus(&this.list_focus, cx),
                InputEvent::Changed => cx.notify(),
            },
        )
        .detach();
        let filter = cx.new(|cx| {
            let mut input = TextInput::new(
                "Filter tasks, or a query like tag:docs",
                quick_add_colors(&palette),
                cx,
            );
            input.set_text(ui.filter.clone(), cx);
            input
        });
        cx.subscribe_in(
            &filter,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Changed => {
                    this.ui.filter = input.read(cx).content.to_string();
                    this.refresh(cx);
                }
                InputEvent::Submit => window.focus(&this.list_focus, cx),
                InputEvent::Cancel => {
                    input.update(cx, |i, cx| i.reset(window, cx));
                    this.ui.filter.clear();
                    this.refresh(cx);
                    window.focus(&this.list_focus, cx);
                }
            },
        )
        .detach();
        let edit = Editors::new(&palette, window, cx);
        let painted_hue = shell.room.hue;
        let mut this = Self {
            store,
            ui,
            shell,
            theme,
            palette,
            painted_hue,
            landscape: HeaderCache::default(),
            quick_add,
            list_focus: cx.focus_handle(),
            rows: UniformListScrollHandle::new(),
            handover_scroll: ScrollHandle::new(),
            handover_bar: kit::Scrollbar::default(),
            rows_bar: kit::Scrollbar::default(),
            sidebar_scroll: ScrollHandle::new(),
            sidebar_bar: kit::Scrollbar::default(),
            detail_scroll: ScrollHandle::new(),
            detail_bar: kit::Scrollbar::default(),
            caption_preview: kit::CaptionPreview::Live,
            menu: None,
            inline: None,
            filter,
            edit,
            motion: Motion::default(),
            clock: Clock::Live(Instant::now()),
            frame: Duration::ZERO,
            row_lifts: Vec::new(),
            alarm: None,
            notice: None,
            width: Width::of(f32::from(window.viewport_size().width)),
            outgoing: None,
            closing: None,
            closing_groups: HashMap::new(),
            animate: true,
        };
        this.load_detail(cx);
        this.settle_detail();
        let _ = this.follow_shell();
        this.watch(cx);
        this
    }

    /// Live data: re-derive when the sync engine reports a change (coalesced to one frame), and every
    /// half minute so relative dates and My Day roll over.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.store.session() else {
            return;
        };
        let mut changes = session.client().changes();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                changes.next().await;
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Re-derives the shell from the store and the UI state.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_with_collapse_change(false, cx);
    }

    fn refresh_with_collapse_change(&mut self, collapsed_changed: bool, cx: &mut Context<Self>) {
        let state = self.store.view();
        // Release the old room's row holds *before* deriving the new counts and sidebar.
        if self.ui.room.resolved(&state) != self.shell.room.key {
            self.motion.settle_all();
        }
        self.follow_data(&state);
        let session = self.store.session();
        // Prune only once the stream is live and caught up: during catch-up after a new or rebuilt
        // cache, a collection from a later page isn't in the replica yet. Stale IDs are harmless
        // meanwhile, since deriving the sidebar only asks whether an ID is collapsed.
        let can_prune =
            session.is_none_or(|s| s.client().status().connection == Connection::Online);
        let pruned = can_prune && self.ui.prune_collapsed_collections(&state);
        if (collapsed_changed || pruned)
            && let Some(session) = session
            && let Err(e) = session
                .data()
                .save_collapsed(&self.ui.collapsed_collections)
        {
            tracing::warn!("couldn't save sidebar preferences: {e}");
            self.notice = Some(Notice::error("Couldn't save sidebar preferences."));
        }
        let shell = model::derive(&state, &self.ui, &self.store.sync(), &self.store.now());
        // A room that disappeared (deleted elsewhere) falls back to My Day; keep the UI state honest.
        self.ui.room = shell.room.key;
        if shell.detail.is_none() {
            self.ui.task = None;
        }
        if shell.room.key != self.shell.room.key {
            self.start_room_switch(&shell);
        } else if self.animate && shell.room.hue != self.shell.room.hue {
            let (now, pace) = (self.now(), self.pace());
            self.motion
                .navigation
                .aim_hue(self.shell.room.hue, shell.room.hue, now, pace);
        }
        if shell.room.quick_add_placeholder != self.shell.room.quick_add_placeholder {
            let placeholder = shell.room.quick_add_placeholder.clone();
            self.quick_add.update(cx, |i, cx| {
                i.placeholder = placeholder;
                cx.notify();
            });
        }
        let old_task = self.shell.detail.as_ref().map(|d| d.task);
        let new_task = shell.detail.as_ref().map(|d| d.task);
        if old_task != new_task {
            if new_task.is_none() {
                self.closing = self.shell.detail.take();
            } else {
                self.closing = None;
                self.detail_scroll.set_offset(gpui::point(px(0.), px(0.)));
            }
        }
        if self.animate {
            self.aim_sidebar(&shell);
        }
        self.shell = shell;
        if self.animate {
            self.aim_detail();
        } else {
            self.settle_detail();
        }
        if self.follow_shell() {
            // A held task the list no longer shows was let go: derive again without it. That pass lets
            // nothing else go, since only shown rows are left.
            return self.refresh_with_collapse_change(false, cx);
        }
        self.load_detail(cx);
        cx.notify();
    }

    /// The motion clock's time now.
    fn now(&self) -> Duration {
        match self.clock {
            Clock::Live(epoch) => epoch.elapsed(),
            Clock::Frozen(at) => at,
        }
    }

    /// How fast transitions starting now run: Windows' *Animation effects* setting, read each time (it's
    /// cheap), so a change made while the app runs applies to the next one. The fixtures use full motion.
    fn pace(&self) -> Pace {
        if self.animate && win::reduced_motion() {
            Pace::REDUCED
        } else {
            Pace::FULL
        }
    }

    /// Before deriving: a choreography follows the data when it changed underneath (another device, an
    /// agent, a refused command), and the window's state holds back exactly the rows the motion does.
    fn follow_data(&mut self, state: &todo_core::State) {
        let (now, pace) = (self.now(), self.pace());
        self.motion.reconcile(now, pace, |id| {
            (state.tasks.get(&id))
                .filter(|t| t.deleted.is_none() && t.merged_into.is_none())
                .map(|t| t.is_done())
        });
        self.ui.holding = self.motion.holding().collect();
        let reopening: Vec<TaskId> = self.motion.reopening().collect();
        self.ui.reopening.retain(|id, _| reopening.contains(id));
    }

    /// After deriving: the sun heads for the new count (the dusk scene takes it at once), and rows the
    /// list doesn't show stop moving (see `Motion::retain_shown`). Returns whether that let a held task
    /// go, so the shell must be derived again.
    fn follow_shell(&mut self) -> bool {
        let now = self.now();
        let room = &self.shell.room;
        let pace = self.pace();
        self.motion
            .aim_dusk(room.key, room.finished_day(), now, pace);
        self.motion
            .aim_sun(room.key, sun_at(room.done, room.total), now, pace);
        let shown: std::collections::HashSet<TaskId> = self.shell.rows().map(|r| r.id).collect();
        self.motion.retain_shown(now, |id| shown.contains(&id))
    }

    /// At the start of each frame: releases rows that have finished leaving (re-deriving the list, the
    /// counts and the sun's target), then asks for exactly the frames the motion needs: the next one while
    /// something moves, a single timer while rows only hold still, and nothing at rest.
    fn drive_motion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.now();
        if self.animate && self.motion.due(now) && !self.motion.advance(now).is_empty() {
            self.refresh(cx);
        }
        // A release can derive dusk and its accompanying detail exit above; sample both at the
        // same frame time, never before the start just written by refresh.
        let now = self.now();
        self.frame = now;
        self.paint_palette(cx);
        let nav = self.motion.navigation;
        if nav.title(now).outgoing == 0. && nav.list(now).outgoing == 0. {
            self.outgoing = None;
        }
        if self.motion.detail.look(now).task == 0. {
            self.closing = None;
        }
        self.motion.sidebar.prune(now);
        self.closing_groups
            .retain(|&id, _| self.motion.sidebar.group(id, false, now).0 > 0.);
        self.measure_lifts();
        self.anchor_scroll();
        if !self.animate {
            return;
        }
        match self.motion.wake(now) {
            Wake::Idle => {}
            Wake::Frame => window.request_animation_frame(),
            Wake::At(at) if self.alarm == Some(at) => {}
            Wake::At(at) => {
                self.alarm = Some(at);
                let wait = at.saturating_sub(now);
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |v, cx| {
                        if v.alarm == Some(at) {
                            v.alarm = None;
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Near the end of a scrolled list, collapsing rows bring the scroll back with them (see
    /// `motion::anchored_scroll`), so the list's end stays put and releasing the slot doesn't jump.
    fn anchor_scroll(&mut self) {
        if self.row_lifts.is_empty() {
            return;
        }
        let frame = self.frame;
        let heights: Vec<f32> = (self.shell.rows())
            .map(|r| self.motion.row(r.id, frame).map_or(1., |l| l.height))
            .collect();
        let state = self.rows.0.borrow();
        let handle = &state.base_handle;
        let viewport = f32::from(handle.bounds().size.height);
        let offset = handle.offset();
        if viewport <= 0. {
            return;
        }
        if let Some(scroll) =
            anchored_scroll(-f32::from(offset.y), viewport, desktop::ROW, &heights)
        {
            handle.set_offset(gpui::point(offset.x, px(-scroll)));
        }
    }

    /// Each row's lift this frame (see `motion::lifts`), or none when nothing is collapsing or entering.
    fn measure_lifts(&mut self) {
        self.row_lifts.clear();
        if !self.motion.rows_moving() {
            return;
        }
        let frame = self.frame;
        let heights: Vec<f32> = self
            .shell
            .rows()
            .map(|r| self.motion.row(r.id, frame).map_or(1., |l| l.height))
            .collect();
        if heights.iter().any(|&h| h < 1.) {
            self.row_lifts = lifts(heights);
        }
    }

    /// Freezes a screenshot fixture at a moment of a transition (see `fixtures::Moment`), so the gallery
    /// can show its frames. The view stays still: it never asks for frames.
    pub fn freeze(&mut self, moment: Moment, window: &Window, cx: &mut Context<Self>) {
        self.motion = Motion::default();
        self.clock = Clock::Frozen(Duration::ZERO);
        let at = match moment {
            Moment::Completing { task, at, end } => {
                self.motion.complete(task, Duration::ZERO, Pace::FULL);
                if end {
                    let last = self.shell.row_count().saturating_sub(1);
                    self.rows.scroll_to_item(last, gpui::ScrollStrategy::Bottom);
                }
                at
            }
            Moment::Reopening {
                task,
                completed,
                at,
            } => {
                self.motion.reopen(task, Duration::ZERO, Pace::FULL);
                self.ui.reopening.insert(task, completed);
                at
            }
            Moment::SunTravel { from, at } => {
                let room = self.shell.room.key;
                let (done, total) = from;
                let place = sun_at(done, total).unwrap_or_default();
                self.motion.place_sun(room, place, Duration::ZERO);
                at
            }
            Moment::Dusk { at } => {
                let room = self.shell.room.key;
                self.motion
                    .aim_dusk(room, false, Duration::ZERO, Pace::FULL);
                at
            }
            Moment::RoomSwitch { at, .. }
            | Moment::DetailOpen { at }
            | Moment::CollectionCollapse { at }
            | Moment::ScrollbarFade { at }
            | Moment::DetailDusk { at, .. } => at,
        };
        self.refresh(cx);
        match moment {
            Moment::RoomSwitch { from, .. } => {
                let ui = UiState {
                    room: from,
                    task: None,
                    ..self.ui.clone()
                };
                let old = model::derive(
                    &self.store.view(),
                    &ui,
                    &self.store.sync(),
                    &self.store.now(),
                );
                let next = std::mem::replace(&mut self.shell, old);
                self.motion = Motion::default();
                let _ = self.follow_shell();
                self.begin_navigation(&next, f32::from(window.viewport_size().height));
                self.aim_sidebar(&next);
                self.shell = next;
                self.settle_detail();
            }
            Moment::CollectionCollapse { .. } => {
                let before = UiState {
                    collapsed_collections: Default::default(),
                    ..self.ui.clone()
                };
                let open = model::derive(
                    &self.store.view(),
                    &before,
                    &self.store.sync(),
                    &self.store.now(),
                );
                let closed = std::mem::replace(&mut self.shell, open);
                self.aim_sidebar(&closed);
                self.shell = closed;
            }
            Moment::ScrollbarFade { at } => self.sidebar_bar.freeze_fade(at),
            Moment::DetailOpen { .. } => {
                self.motion.detail = DetailMotion::rest(None, self.width == Width::Wide);
                self.aim_detail();
            }
            Moment::DetailDusk { task, .. } => {
                let ui = UiState {
                    task: Some(task),
                    holding: [task].into(),
                    ..self.ui.clone()
                };
                let before = model::derive(
                    &self.store.view(),
                    &ui,
                    &self.store.sync(),
                    &self.store.now(),
                );
                self.closing = before.detail;
                self.motion = Motion::default();
                self.motion.detail = DetailMotion::rest(Some(task), self.width == Width::Wide);
                let room = self.shell.room.key;
                self.motion
                    .aim_dusk(room, false, Duration::ZERO, Pace::FULL);
                let _ = self.follow_shell();
                self.aim_detail();
            }
            _ => {}
        }
        self.clock = Clock::Frozen(Duration::from_millis(u64::from(at)));
    }

    fn aim_detail(&mut self) {
        self.motion.detail.aim(
            self.ui.task,
            self.detail_placement() == DetailPane::Docked,
            self.width == Width::Narrow,
            self.now(),
            self.pace(),
        );
    }

    fn settle_detail(&mut self) {
        self.motion.detail = DetailMotion::rest(
            self.shell.detail.as_ref().map(|d| d.task),
            self.detail_placement() == DetailPane::Docked,
        );
        self.closing = None;
    }

    fn paint_palette(&mut self, cx: &mut Context<Self>) {
        let hue = self.motion.navigation.hue(self.frame, self.shell.room.hue);
        if hue == self.painted_hue && self.palette.theme == self.theme {
            return;
        }
        self.painted_hue = hue;
        self.palette = theme::palette(self.theme, hue);
        let colors = quick_add_colors(&self.palette);
        for input in [&self.quick_add, &self.filter, &self.edit.step] {
            input.update(cx, |i, cx| {
                i.colors = colors;
                cx.notify();
            });
        }
        for area in [&self.edit.title, &self.edit.notes] {
            area.update(cx, |a, cx| {
                a.colors = colors;
                cx.notify();
            });
        }
    }

    fn start_room_switch(&mut self, next: &Shell) {
        if self.animate {
            self.begin_navigation(next, 0.);
        }
        self.rows
            .0
            .borrow()
            .base_handle
            .set_offset(gpui::point(px(0.), px(0.)));
        self.rows.scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.handover_scroll.set_offset(gpui::point(px(0.), px(0.)));
    }

    fn begin_navigation(&mut self, next: &Shell, capture_height: f32) {
        let now = self.now();
        let pace = self.pace();
        let keep = self
            .motion
            .navigation
            .switch(self.shell.room.hue, next.room.hue, now, pace);
        // The title enters later than the list: their dominant layers may be different rooms.
        let old = self.outgoing.take();
        let (title, eyebrow) = match &old {
            Some(old) if keep.title => (old.title.clone(), old.eyebrow.clone()),
            _ => (
                self.shell.room.title.clone(),
                self.shell.room.eyebrow.clone(),
            ),
        };
        let body = old
            .filter(|_| keep.list)
            .map_or_else(|| self.capture_body(capture_height), |old| old.body);
        self.outgoing = Some(OutgoingRoom {
            title,
            eyebrow,
            body,
        });
        self.motion
            .navigate_dusk(next.room.key, next.room.finished_day(), now, pace);
        self.motion.navigate_sun(
            next.room.key,
            sun_at(next.room.done, next.room.total),
            now,
            pace,
        );
    }

    /// Where a screenshot fixture puts the keyboard: the quick-add when it has typed text, else the list,
    /// as in the running app.
    pub fn fixture_focus(&self, cx: &App) -> FocusHandle {
        let quick_add = self.quick_add.read(cx);
        if quick_add.content.is_empty() {
            self.list_focus.clone()
        } else {
            quick_add.focus_handle.clone()
        }
    }

    pub fn store(&self) -> &Rc<Store> {
        &self.store
    }

    /// My Day's progress, for the quick-add window's sun.
    pub fn my_day_progress(&self) -> (u32, u32) {
        model::my_day_progress(&self.store.view(), &self.store.now())
    }

    fn submit(&mut self, cmds: Vec<todo_core::Command>) -> bool {
        for cmd in cmds {
            if let Err(e) = self.store.submit(cmd) {
                tracing::warn!("a change was refused: {e}");
                self.notice = Some(Notice::error(format!("Couldn't save that: {e}")));
                return false;
            }
        }
        self.notice = None;
        true
    }

    /// Shows a line in the status bar (until the next change replaces it).
    pub fn show_notice(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(if error {
            Notice::error(text)
        } else {
            Notice::info(text)
        });
        cx.notify();
    }

    /// Shows a warning in the status bar with an "Open folder" action for `folder`.
    pub fn show_folder_notice(
        &mut self,
        text: impl Into<SharedString>,
        folder: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(Notice {
            folder: Some(folder),
            ..Notice::error(text)
        });
        cx.notify();
    }

    /// A draft couldn't be saved, so the task stays open with its text: say so, with the reason.
    fn keep_unsaved(&mut self, cx: &mut Context<Self>) {
        let why = self
            .notice
            .as_ref()
            .filter(|n| n.error)
            .map(|n| n.text.to_string())
            .unwrap_or_default();
        self.notice = Some(Notice::error(
            format!("{why} Your text is kept here; it's saved when you try again.")
                .trim_start()
                .to_string(),
        ));
        cx.notify();
    }

    /// Adds a task typed into a quick-add, to `room`. Returns whether it was added.
    pub fn add_task(&mut self, text: &str, room: RoomKey, cx: &mut Context<Self>) -> bool {
        let now = self.store.now();
        let parsed = quick_add::parse(text, &now.eval());
        let store = self.store.clone();
        let (id, cmds) = commands::add_task(&store.view(), room, &parsed, now.today(), || {
            store.new_uuid()
        });
        let ok = id.is_some() && self.submit(cmds);
        self.refresh(cx);
        ok
    }

    pub fn select_room(&mut self, room: RoomKey, cx: &mut Context<Self>) {
        if !self.commit_edits(cx) && self.ui.room != room {
            self.keep_unsaved(cx);
            return;
        }
        let revealed = self.ui.select_room(&self.store.view(), room);
        self.refresh_with_collapse_change(revealed, cx);
    }

    fn toggle_collection(&mut self, id: CollectionId, cx: &mut Context<Self>) {
        let live = self.store.view().is_live(id.into());
        if live {
            self.ui.toggle_collection(id);
        }
        self.refresh_with_collapse_change(live, cx);
    }

    fn select_task(&mut self, id: Option<TaskId>, cx: &mut Context<Self>) {
        if id.is_some_and(|id| !self.task_is_visible(id)) {
            return;
        }
        if self.ui.task != id && !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        self.ui.task = id;
        self.refresh(cx);
        if let Some(i) = self.shell.selected_task {
            self.rows.scroll_to_item(i, gpui::ScrollStrategy::Center);
        } else if let (Some(next), Some(id)) = (self.shell.room.handover(), id)
            && let Some(child) = next.child(id)
        {
            self.handover_scroll.scroll_to_item(child);
        }
    }

    /// Completes or reopens a task. The command applies at once; the row then plays the choreography
    /// (`model::motion`): a completed task fills, holds its place struck through, collapses and only then
    /// leaves the open rows, and the counts and the sun follow. A reopened one clears and leaves its
    /// completed place for the open rows. Toggling it again part way reverses it from where it is.
    fn toggle_complete(&mut self, id: TaskId, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(task) = state.tasks.get(&id) else {
            return;
        };
        let (was_done, completed_at) = (task.is_done(), task.completed_at);
        let Some(cmd) = commands::toggle_complete(&state, id) else {
            return;
        };
        if self.submit(vec![cmd]) && self.animate {
            let (now, pace) = (self.now(), self.pace());
            let keep = if was_done {
                self.motion.reopen(id, now, pace)
            } else {
                self.motion.complete(id, now, pace)
            };
            if keep == Keep::Reopening
                && let Some(at) = completed_at
            {
                self.ui.reopening.insert(id, at);
            }
        }
        self.refresh(cx);
    }

    /// Moves the selected task up or down its list (only where the order is the list's own).
    fn move_task(&mut self, by: i32, cx: &mut Context<Self>) {
        let (Some(id), RoomKey::List(_)) = (self.ui.task, self.ui.room) else {
            return;
        };
        if let Some(cmd) = commands::nudge(&self.store.view(), id.into(), by) {
            self.submit(vec![cmd]);
        }
        self.select_task(Some(id), cx);
    }

    fn cycle_priority(&mut self, id: TaskId, cx: &mut Context<Self>) {
        if let Some(cmd) = commands::cycle_priority(&self.store.view(), id) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// J and K: through the rows, or through the finished day's hand-over to tomorrow.
    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let tasks = self.nav_tasks();
        let n = tasks.len();
        if n == 0 {
            return;
        }
        let at = self
            .ui
            .task
            .and_then(|id| tasks.iter().position(|&t| t == id));
        let next = match at {
            None if by > 0 => 0,
            None => n - 1,
            Some(i) => i.saturating_add_signed(by).min(n - 1),
        };
        self.select_task(Some(tasks[next]), cx);
    }

    pub fn preview_caption(&mut self, preview: kit::CaptionPreview) {
        self.caption_preview = preview;
    }

    /// Crossing the breakpoint is a resize, not navigation; don't replay the pane's entrance.
    fn resize(&mut self, width: Width) {
        if self.width != width {
            self.width = width;
            self.settle_detail();
        }
    }

    /// Draws every pane at rest, without transitions: for the screenshot fixtures.
    pub fn still(&mut self) {
        self.animate = false;
        self.settle_detail();
    }

    fn titlebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let filtering = !self.ui.filter.is_empty();
        let focused = self.filter.read(cx).focus_handle.is_focused(window);
        div()
            .h(px(desktop::TITLEBAR))
            .flex_none()
            .flex()
            .items_center()
            .bg(p.canvas)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .pl(px(space::S4))
                    .window_control_area(WindowControlArea::Drag)
                    .child(kit::icon(
                        Icon::CircleCheck,
                        component::ICON,
                        p.accent_strong,
                    ))
                    .child(
                        div()
                            .style(ty::NAV_COLLECTION.with_weight(650))
                            .text_color(p.ink)
                            .child("To-do"),
                    ),
            )
            .child(
                div()
                    // The search field is centred in the title bar at the mockup width.
                    .w(px(440.))
                    .h(px(desktop::TITLEBAR - space::S2 - space::S1 / 2.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .pl(px(space::S2))
                    .pr(px(space::S1))
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_color(if focused || filtering {
                        p.accent
                    } else {
                        p.line_strong
                    })
                    .bg(p.pane)
                    .child(kit::icon(
                        Icon::Search,
                        component::ICON_SMALL,
                        if filtering { p.accent_strong } else { p.ink_3 },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .style(ty::SUMMARY)
                            .child(self.filter.clone()),
                    )
                    .child(if filtering {
                        div()
                            .id("clear-filter")
                            .rounded(px(radius::SM))
                            .hover(|s| s.bg(p.hover))
                            .child(kit::icon(Icon::X, component::ICON_SMALL, p.ink_3))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.filter.update(cx, |i, cx| i.reset(window, cx));
                                this.ui.filter.clear();
                                this.refresh(cx);
                            }))
                            .into_any_element()
                    } else {
                        kit::kbd(p, "Ctrl K").into_any_element()
                    }),
            )
            .child(kit::window_controls(p, window, self.caption_preview))
    }

    fn eyebrow(&self, eyebrow: &Eyebrow) -> Div {
        let (ink_2, ink_3, line) = self.header_inks();
        let bar = || {
            div()
                .w(px(elevation::HAIRLINE))
                .h(px(ty::CAPS_LABEL.size - 0.5))
                .bg(line)
        };
        let mono = |s: &SharedString| {
            div()
                .style(ty::EYEBROW_META)
                .text_color(ink_3)
                .child(s.to_uppercase())
        };
        let dow = |s: SharedString| div().style(ty::EYEBROW_DAY).text_color(ink_2).child(s);
        let row = div()
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .h(px(ty::EYEBROW_DAY.line_height));
        match eyebrow {
            Eyebrow::Day {
                weekday,
                date,
                week,
            } => row
                .child(dow(weekday.clone()))
                .child(bar())
                .child(mono(date))
                .child(bar())
                .child(mono(week)),
            Eyebrow::Kind { kind, detail } => row
                .child(dow(kind.clone()))
                .child(bar())
                .child(mono(detail)),
        }
    }

    fn summary(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let (done, total, open) = room.summary();
        let (ink_2, _, line) = self.header_inks();
        let finished_day = room.finished_day();
        let ink = if finished_day { ink_2 } else { p.ink };
        let strong = if finished_day { ink_2 } else { p.accent_strong };
        let sep = || {
            div()
                .size(px(elevation::SELECTION_BAR))
                .rounded_full()
                .bg(line)
        };
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .pb(px(space::S3))
            .style(ty::SUMMARY)
            .text_color(ink_2);
        if total == 0 {
            let finished = room.finished_this_month;
            return row.child(div().style(ty::SUMMARY_FIGURES).child(if finished > 0 {
                format!("{finished} finished this month")
            } else {
                "nothing yet".to_string()
            }));
        }
        row = row
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .style(ty::SUMMARY_FIGURES.with_weight(600))
                            .text_color(ink)
                            .child(done.to_string()),
                    )
                    .child(format!("of {total} done")),
            )
            .child(sep())
            .child(if finished_day {
                // The day is done: the count says so in the accent, without congratulations.
                div()
                    .style(ty::SUMMARY_FIGURES.with_weight(600))
                    .text_color(self.dusk_accent())
                    .child("all clear")
            } else {
                div()
                    .style(ty::SUMMARY_FIGURES)
                    .child(format!("{open} open"))
            });
        if let Some(est) = &room.estimate {
            row = row.child(sep()).child(
                div()
                    .style(ty::SUMMARY_FIGURES.with_weight(600))
                    .text_color(strong)
                    .child(format!("est. {est}")),
            );
        }
        row
    }

    /// (secondary ink, metadata ink, separator) for text on the header. The dusk scene is a night scene in
    /// both themes, so its header uses the dark theme's inks.
    fn header_inks(&self) -> (Hsla, Hsla, Hsla) {
        let p = if self.motion.dusk(self.frame).night_ink() {
            theme::palette(Theme::Dark, self.painted_hue)
        } else {
            self.palette
        };
        (p.ink_2, p.ink_3, p.line_strong)
    }

    /// The accent on the dusk scene: the dark theme's, since the scene is a night scene.
    fn dusk_accent(&self) -> Hsla {
        theme::palette(Theme::Dark, self.painted_hue).accent_strong
    }

    fn header(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        // The everyday strip, the finished day's dusk scene, or the dusk falling or lifting between them.
        let look = self.motion.dusk(self.frame);
        let dusk = theme::dusk(self.painted_hue);
        let sky_bg = theme::mix(p.sky, dusk.top, look.scene());
        let title_ink = if look.night_ink() {
            theme::palette(Theme::Dark, self.painted_hue).ink
        } else {
            p.ink
        };
        let blend = Blend {
            height: look.height(),
            scene: look.scene(),
            night: look.night(),
            sun_alpha: self.motion.sun_alpha(self.frame),
        };
        let (_, ink_3, _) = self.header_inks();
        // Long titles end in an ellipsis from text layout, with no clipping: the display face's
        // descenders (the y in "My Day") hang below its tight line box, and GPUI clips both axes even
        // for `overflow_x_hidden`.
        let title_text = |text: SharedString| {
            div()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .style(ty::ROOM_TITLE)
                .text_color(title_ink)
                .child(text)
        };
        let swap = self.motion.navigation.title(self.frame);
        let old = self.outgoing.as_ref().filter(|_| swap.outgoing > 0.);
        let title = div()
            .relative()
            .min_w_0()
            .flex_1()
            .child(
                title_text(room.title.clone())
                    .relative()
                    .top(px(swap.incoming_y))
                    .opacity(swap.incoming),
            )
            .children(old.map(|old| {
                title_text(old.title.clone())
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(swap.outgoing_y))
                    .opacity(swap.outgoing)
            }));
        let eyebrow = div()
            .relative()
            .child(
                self.eyebrow(&room.eyebrow)
                    .relative()
                    .top(px(swap.incoming_y))
                    .opacity(swap.incoming),
            )
            .children(old.map(|old| {
                self.eyebrow(&old.eyebrow)
                    .absolute()
                    .left_0()
                    .top(px(swap.outgoing_y))
                    .opacity(swap.outgoing)
            }));
        let action = |icon: Icon| {
            div()
                .size(px(component::ICON_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .child(kit::icon(icon, component::ICON, ink_3))
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(sky_bg)
            .child(
                div()
                    .pt(px(desktop::HEADER_PAD_TOP))
                    .pb(px(space::S2))
                    .px(px(desktop::LIST_PAD_X))
                    .child(
                        div()
                            .h(px(desktop::EYEBROW))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(eyebrow)
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S1))
                                    .mr(px(-space::S2))
                                    .child(action(Icon::SlidersHorizontal))
                                    .child(action(Icon::Ellipsis)),
                            ),
                    )
                    .child(if self.width.counts_under_title() {
                        // Narrow: the title keeps the whole line, and the counts go under it.
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(title)
                            .child(self.summary())
                    } else {
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(space::S6))
                            .child(title)
                            .child(div().flex_none().child(self.summary()))
                    }),
            )
            // The finished scene's rings reach past the strip; the header ends at its bottom edge.
            .child(div().overflow_hidden().child(header_landscape(
                &self.landscape,
                self.palette,
                dusk,
                self.motion.sun(self.frame),
                blend,
            )))
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
    }

    fn quick_add_field(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        let input = self.quick_add.read(cx);
        let focused = input.focus_handle.is_focused(window);
        let text = input.content.clone();
        let typed = !text.is_empty();
        // On the finished day the field stays, but quietly: on the pane with a hairline, so the hand-over
        // to tomorrow leads. Ctrl+N or a click wakes it up.
        let quiet = self.shell.room.finished_day() && !focused && !typed;
        let field = div()
            .h(px(desktop::QUICK_ADD))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S3 + 2.))
            .pr(px(space::S2 + 2.))
            .rounded(px(radius::QUICK_ADD))
            .border(px(elevation::HAIRLINE))
            .border_color(if focused || typed {
                p.accent
            } else if quiet {
                p.line
            } else {
                p.control
            })
            .bg(if quiet { p.pane } else { p.raised })
            .style(ty::QUICK_ADD)
            .child(kit::icon(
                Icon::Plus,
                component::ICON,
                if quiet { p.ink_3 } else { p.accent_strong },
            ))
            .child(div().flex_1().min_w_0().child(self.quick_add.clone()));
        let field = if typed {
            // What the typed words will set: the parsed due date as a chip.
            let now = self.store.now();
            let parsed = quick_add::parse(&text, &now.eval());
            field
                .children(parsed.due.map(|d| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(component::CHIP_GAP))
                        .px(px(space::S2))
                        .h(px(component::CHIP + 2.))
                        .rounded(px(radius::SM))
                        .bg(p.wash)
                        .text_color(p.accent_strong)
                        .style(ty::COUNT.with_weight(500))
                        .child(kit::icon(
                            Icon::Calendar,
                            component::ICON_SMALL,
                            p.accent_strong,
                        ))
                        .child(model::labels::short_date(d, now.today()))
                }))
                .child(kit::kbd(p, "Enter"))
        } else if quiet {
            field.child(kit::kbd(p, "Ctrl N"))
        } else {
            field
                .child(kit::icon(Icon::Calendar, component::GLYPH, p.ink_3))
                .child(kit::icon(Icon::Flag, component::GLYPH, p.ink_3))
                .child(kit::kbd(p, "Ctrl N"))
        };
        // The focus ring is a band of the room tint around the field (a border, not a shadow).
        let ring = if focused || typed {
            p.tint
        } else {
            gpui::transparent_black()
        };
        div()
            .mx(px(desktop::LIST_PAD_X - elevation::FOCUS_RING))
            .my(px(desktop::QUICK_ADD_MARGIN - elevation::FOCUS_RING))
            .p(px(elevation::FOCUS_RING))
            .rounded(px(radius::QUICK_ADD + elevation::FOCUS_RING))
            .bg(ring)
            .child(field)
    }

    fn due(p: &Palette, due: &Due) -> AnyElement {
        match due {
            Due::None => div()
                .style(ty::DUE)
                .text_color(p.ink_4)
                .child("–")
                .into_any_element(),
            Due::Soon(s) => div()
                .style(ty::DUE.with_weight(600))
                .text_color(p.accent_strong)
                .child(s.clone())
                .into_any_element(),
            Due::Plain(s) => div()
                .style(ty::DUE)
                .text_color(p.ink_2)
                .child(s.clone())
                .into_any_element(),
            Due::Overdue(s) => kit::overdue_badge(p, s.clone()).into_any_element(),
        }
    }

    /// A row in its slot of the list. While it plays its choreography it fades and collapses (or grows)
    /// inside the slot, and the rows below rise by what the rows above them have given up, so the gap
    /// closes smoothly. The slots keep the list's fixed row height until the row is released.
    fn row_element(
        &self,
        i: usize,
        task: &TaskRow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = self.motion.row(task.id, self.frame);
        let lift = self.row_lifts.get(i).copied().unwrap_or(0.);
        let visible = self.row_is_visible(task.id);
        let style = RowStyle {
            room: visible.then_some(self.shell.room.key),
            selected: self.shell.selected_task == Some(i),
            chips: self.shell.room.chips,
            look,
        };
        let row = self.row(task, style, window, cx);
        if look.is_none() && lift == 0. {
            return row.into_any_element();
        }
        let (height, opacity) = look.map_or((1., 1.), |l| (l.height, l.opacity));
        div()
            .w_full()
            .h(px(desktop::ROW))
            .flex_none()
            .child(
                div()
                    .relative()
                    .top(px(-lift * desktop::ROW))
                    .w_full()
                    .h(px(desktop::ROW * height))
                    .overflow_hidden()
                    .opacity(opacity)
                    .child(row),
            )
            .into_any_element()
    }

    /// Where the detail pane goes: docked beside the room, over it (a narrow window), or nowhere (see
    /// `DetailPane::at`).
    fn detail_placement(&self) -> DetailPane {
        DetailPane::at(
            self.width,
            self.shell.detail.is_some(),
            self.shell.room.finished_day(),
        )
    }

    fn docked(&self) -> bool {
        self.width == Width::Wide && self.motion.detail.look(self.frame).docked
    }

    /// The room pane's width (and so a row's): the window less the sidebar and a docked detail pane.
    fn room_width(&self, window: &Window) -> f32 {
        let pane = if self.docked() {
            DetailPane::Docked
        } else {
            DetailPane::Hidden
        };
        layout::room_width(f32::from(window.viewport_size().width), pane)
    }

    /// The narrow window's detail: over the right part of the room, with a scrim over the rest of it.
    /// It slides in from the right and fades in (`OVERLAY_IN`), unless the window narrowed with the task
    /// already open, and fades out when closed (`OVERLAY_OUT`); with reduced motion it only fades,
    /// briefly. Clicking the scrim closes it and leaves the keyboard on the list. A pane fading out is
    /// inert: a shield over it swallows every click and hover, so nothing in it acts on the dismissed
    /// task, and the scrim still keeps clicks off the rows underneath.
    fn detail_overlay(&self, window: &Window, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let look = self.motion.detail.look(self.frame);
        if self.width != Width::Narrow || look.task == 0. {
            return None;
        }
        let p = self.palette;
        let live = self.shell.detail.is_some();
        let scrim = div()
            .id(if live { "scrim" } else { "scrim-closing" })
            .absolute()
            .inset_0()
            .bg(p.scrim)
            .opacity(look.task)
            .occlude()
            .when(live, |s| {
                s.on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.list_focus, cx);
                    this.select_task(None, cx);
                }))
            });
        let pane = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(-look.task_x))
            .opacity(look.task)
            // Rows under the pane take no clicks or hovers. Blocking them also blocks the shell's own
            // mouse-down, so the pane leaves the keyboard on the list itself, as the shell does.
            .occlude()
            .when(live, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !window.default_prevented() {
                            window.focus(&this.list_focus, cx);
                        }
                    }),
                )
            })
            .shadow(kit::overlay_shadow(&p))
            .child(
                self.detail_pane(window, cx)
                    .when(p.night(), |d| d.border_color(p.line_strong)),
            );
        Some(vec![scrim.into_any_element(), pane.into_any_element()])
    }

    /// A row's metadata line: the list chip, steps, the mail mark, then tags. Every item is measured and
    /// fitted to the title column's width (`room`, see `fit::fit_meta`): tags that don't fit whole are
    /// counted as "+N", and when the line is narrower still, the mail mark, the step meter and the step
    /// count give way in turn before the chip narrows to its minimum, ending in an ellipsis. The line is
    /// never wider than its column and nothing is clipped, so no descender is either.
    fn meta(&self, task: &TaskRow, chips: Chips, room: f32, window: &Window) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let with_collection = match chips {
            Chips::None => None,
            Chips::List => Some(false),
            Chips::CollectionAndList => Some(true),
        };
        let count_style = ty::COUNT.with_weight(500);
        let steps = task.steps.map(|(d, n)| (d, n, format!("{d}/{n}")));
        let items = fit::MetaItems {
            gap: space::S2,
            chip: with_collection.map(|w| {
                (
                    kit::chip_width(window, &task.list, w),
                    kit::chip_min_width(window),
                )
            }),
            steps: steps.as_ref().map(|(_, n, label)| {
                (
                    kit::meter_width(*n),
                    component::CHIP_GAP,
                    kit::text_width(window, count_style, label),
                )
            }),
            mail: task.from_mail.then_some(component::ICON_SMALL),
            tags: task
                .tags
                .iter()
                .map(|tag| kit::text_width(window, ty::TAG, tag))
                .collect(),
        };
        let more_width = |n: usize| kit::text_width(window, ty::TAG, &fit::more_label(n));
        let fit = fit::fit_meta(room, &items, more_width);
        let tag = |s: SharedString| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .style(ty::TAG)
                .text_color(p.ink_3)
                .child(s)
        };
        div()
            .flex()
            .items_center()
            .gap(px(items.gap))
            .h(px(component::CHIP))
            .mt(px(desktop::ROW_LINE_GAP))
            .when_some(fit.chip.zip(with_collection), |m, (width, w)| {
                m.child(kit::chip(t, &task.list, w).w(px(width)))
            })
            .when_some(
                steps.filter(|_| fit.meter || fit.count),
                |m, (d, n, label)| {
                    m.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(component::CHIP_GAP))
                            .text_color(p.ink_3)
                            .style(count_style)
                            .when(fit.meter, |s| s.child(kit::meter(p, d, n)))
                            .when(fit.count, |s| s.child(label)),
                    )
                },
            )
            .when(fit.mail, |m| {
                m.child(kit::icon(Icon::Mail, component::ICON_SMALL, p.ink_3))
            })
            .children(task.tags[..fit.tags.shown].iter().cloned().map(tag))
            .when(fit.tags.more > 0, |m| {
                m.child(tag(fit::more_label(fit.tags.more).into()))
            })
    }

    fn row(
        &self,
        task: &TaskRow,
        style: RowStyle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = &self.palette;
        let t = self.theme;
        let RowStyle {
            selected,
            room,
            look,
            chips,
        } = style;
        let overdue = matches!(task.due, Due::Overdue(_));
        // The trailing columns keep their places from the right edge, but empty ones to the left of every
        // filled one are left out, so the title runs on into their room instead of stopping short.
        let show_agent = task.agent.is_some();
        let show_priority = show_agent || task.priority > 0;
        let full_due = show_priority || !matches!(task.due, Due::None);
        // The title column's width: the row less its padding, the checkbox and the trailing columns.
        let mut title_width =
            self.room_width(window) - 2. * desktop::LIST_PAD_X - desktop::COL_CHECK;
        title_width -= desktop::ROW_GAP
            + if full_due {
                desktop::COL_DUE
            } else {
                kit::text_width(window, ty::DUE, "–")
            };
        if show_priority {
            title_width -= desktop::ROW_GAP + desktop::COL_PRIORITY;
        }
        if show_agent {
            title_width -= desktop::ROW_GAP + desktop::COL_AGENT;
        }
        title_width -= desktop::ROW_GAP;
        let meta = self.meta(task, chips, title_width, window);
        let id = task.id;
        let check = match look {
            Some(l) => kit::checkbox_at(
                p,
                overdue,
                selected,
                desktop::COL_CHECK,
                (l.fill, l.scale, l.tick),
            ),
            None if task.done => kit::checked(p, selected, desktop::COL_CHECK),
            None => kit::checkbox(p, overdue, desktop::COL_CHECK),
        };
        let check = div()
            .id(SharedString::from(format!("check-{id}")))
            .flex_none()
            .child(check)
            .when_some(room, |el, room| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if this.shell.room.key == room && this.task_is_visible(id) {
                        this.toggle_complete(id, cx);
                    }
                }))
            });
        let title = self.title(task, look, title_width, window);
        let row = div()
            .id(SharedString::from(format!("task-{id}")))
            .relative()
            .w_full()
            .h(px(desktop::ROW))
            .flex_none()
            .flex()
            .items_start()
            .gap(px(desktop::ROW_GAP))
            .pt(px(desktop::ROW_PAD_TOP))
            .px(px(desktop::LIST_PAD_X))
            .border_b_1()
            .border_color(p.line)
            .child(check)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(meta),
            )
            .when(show_agent, |row| {
                row.child(
                    div().w(px(desktop::COL_AGENT)).flex_none().children(
                        task.agent
                            .map(|a| kit::agent_mark(t, a, component::AGENT_MARK)),
                    ),
                )
            })
            .when(show_priority, |row| {
                row.child(
                    div()
                        .id(SharedString::from(format!("priority-{id}")))
                        .w(px(desktop::COL_PRIORITY))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(kit::priority(p, task.priority))
                        .when_some(room, |el, room| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if this.shell.room.key == room && this.task_is_visible(id) {
                                    this.cycle_priority(id, cx);
                                }
                            }))
                        }),
                )
            })
            .child(
                div()
                    .when(full_due, |d| d.w(px(desktop::COL_DUE)))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(Self::due(p, &task.due)),
            )
            .when_some(room, |el, room| {
                el.on_click(cx.listener(move |this, _, window, cx| {
                    if this.shell.room.key == room && this.row_is_visible(id) {
                        window.focus(&this.list_focus, cx);
                        this.select_task(Some(id), cx);
                    }
                }))
            });
        if selected {
            row.bg(p.wash).child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(-elevation::HAIRLINE))
                    .bottom(px(-elevation::HAIRLINE))
                    .w(px(elevation::SELECTION_BAR))
                    .bg(p.accent),
            )
        } else {
            let hover = p.hover;
            row.when(room.is_some(), |el| el.hover(move |s| s.bg(hover)))
        }
    }

    /// A row's title: dimmed and struck through when it's done. While it's being completed or reopened
    /// the colour fades and the strike-through draws (or withdraws) from the left, in step with the tick;
    /// at rest it's GPUI's own strike-through, drawn in exactly the same place.
    fn title(&self, task: &TaskRow, look: Option<RowLook>, width: f32, window: &Window) -> Div {
        let p = &self.palette;
        let text = || {
            div()
                .style(ty::TASK_TITLE)
                .truncate()
                .child(task.title.clone())
        };
        let Some(l) = look else {
            return if task.done {
                text().text_color(p.ink_3).line_through()
            } else {
                text().text_color(p.ink)
            };
        };
        let ink = theme::mix(p.ink, p.ink_3, l.dim);
        if l.strike >= 1. {
            return text().text_color(ink).line_through();
        }
        if l.strike <= 0. {
            return text().text_color(ink);
        }
        // The same line again, invisible but for its strike-through, cut off where the stroke has reached.
        let reach = kit::text_width(window, ty::TASK_TITLE, &task.title).min(width);
        let mut strike = text().w(px(width)).text_color(gpui::transparent_black());
        strike.text_style().strikethrough = Some(StrikethroughStyle {
            thickness: px(elevation::HAIRLINE),
            color: Some(ink),
        });
        div().relative().child(text().text_color(ink)).child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(reach * l.strike))
                .overflow_hidden()
                .child(strike),
        )
    }

    fn capture_body(&self, min_height: f32) -> OutgoingBody {
        let room = &self.shell.room;
        if let Some(next) = room.handover() {
            OutgoingBody::Handover {
                tomorrow: Box::new(next.clone()),
                top: self.handover_scroll.offset().y,
            }
        } else if let Some(weekday) = room.plan_only() {
            OutgoingBody::Plan(weekday.clone())
        } else if self.shell.row_count() == 0 {
            OutgoingBody::Empty {
                line: room.empty_line.clone(),
                hint: room.empty_hint.clone(),
            }
        } else {
            let state = self.rows.0.borrow();
            let viewport = f32::from(state.base_handle.bounds().size.height).max(min_height);
            let scroll = (-f32::from(state.base_handle.offset().y)).max(0.);
            let first = (scroll / desktop::ROW).floor() as usize;
            let cut = scroll - first as f32 * desktop::ROW;
            let visible = ((viewport + cut) / desktop::ROW).ceil() as usize;
            let rows = self
                .shell
                .rows()
                .enumerate()
                .skip(first)
                .take(visible)
                .map(|(i, row)| (row.clone(), self.shell.selected_task == Some(i)))
                .collect();
            OutgoingBody::Rows {
                rows,
                cut,
                chips: room.chips,
            }
        }
    }

    fn empty_list(&self, line: &SharedString, hint: &SharedString) -> Div {
        div()
            .px(px(desktop::LIST_PAD_X))
            .pt(px(space::S6 + space::S1))
            .child(
                div()
                    .style(ty::EMPTY_LINE)
                    .text_color(self.palette.ink)
                    .child(line.clone()),
            )
            .child(
                div()
                    .mt(px(space::S2 + 2.))
                    .style(ty::BODY)
                    .text_color(self.palette.ink_3)
                    .child(hint.clone()),
            )
    }

    fn outgoing_list(&self, alpha: f32, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let old = self.outgoing.as_ref().filter(|_| alpha > 0.)?;
        let body = match &old.body {
            OutgoingBody::Rows { rows, cut, chips } => div()
                .relative()
                .top(px(-cut))
                .children(rows.iter().map(|(row, selected)| {
                    self.row(
                        row,
                        RowStyle {
                            room: None,
                            selected: *selected,
                            chips: *chips,
                            look: None,
                        },
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
            OutgoingBody::Empty { line, hint } => self.empty_list(line, hint).into_any_element(),
            OutgoingBody::Handover { tomorrow, top } => {
                self.tomorrow(tomorrow, Some(*top), cx).into_any_element()
            }
            OutgoingBody::Plan(weekday) => self
                .plan_only(weekday.clone(), false, cx)
                .into_any_element(),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .opacity(alpha)
                .child(div().id("outgoing-room").size_full().child(body)),
        )
    }

    fn list(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let room = &self.shell.room;
        // The list's scrollbar shows only while its rows do.
        let swap = self.motion.navigation.list(self.frame);
        let has_rows = self.list_is_visible()
            && !(room.finished_day() && room.completed.is_empty())
            && self.shell.row_count() > 0;
        // The finished day hands over to tomorrow, unless the Completed section is open: then its rows show.
        let body: AnyElement = if let Some(next) = room.handover() {
            self.tomorrow(next, None, cx).into_any_element()
        } else if let Some(weekday) = room.plan_only() {
            self.plan_only(weekday.clone(), true, cx).into_any_element()
        } else if self.shell.row_count() == 0 {
            self.empty_list(&room.empty_line, &room.empty_hint)
                .into_any_element()
        } else {
            let count = self.shell.row_count();
            uniform_list(
                "rows",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                    let rows: Vec<TaskRow> = range
                        .clone()
                        .filter_map(|i| this.shell.row(i).cloned())
                        .collect();
                    let mut items = range
                        .clone()
                        .zip(rows.iter())
                        .map(|(i, r)| this.row_element(i, r, window, cx))
                        .collect::<Vec<_>>();
                    // While rows collapse, the rows below rise; `uniform_list` only renders the slots in
                    // view, so the ones rising into view from below are drawn (and take clicks) with the
                    // last slot it renders.
                    let past: Vec<usize> = rows_rising_into(&this.row_lifts, range.end).collect();
                    if !past.is_empty()
                        && let Some(last) = items.pop()
                    {
                        let rising: Vec<AnyElement> = past
                            .into_iter()
                            .filter_map(|j| {
                                let r = this.shell.row(j)?.clone();
                                let at = (j + 1 - range.end) as f32 * desktop::ROW;
                                Some(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top(px(at))
                                        .w_full()
                                        .child(this.row_element(j, &r, window, cx))
                                        .into_any_element(),
                                )
                            })
                            .collect();
                        items.push(
                            div()
                                .relative()
                                .w_full()
                                .h(px(desktop::ROW))
                                .child(last)
                                .children(rising)
                                .into_any_element(),
                        );
                    }
                    items
                }),
            )
            .track_scroll(&self.rows)
            .size_full()
            .into_any_element()
        };
        // The reserved strip at the bottom: "Completed n" on the left, room for the undo toast on the right.
        let open = self.ui.show_completed;
        let completed = (room.completed_count > 0).then(|| {
            div()
                .id("completed-toggle")
                .h(px(desktop::TOAST_INSET))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(desktop::LIST_PAD_X))
                .border_t_1()
                .border_color(p.line)
                .child(kit::icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    component::ICON_SMALL,
                    p.ink_3,
                ))
                .child(kit::label(&p, "Completed"))
                .child(
                    div()
                        .style(ty::CAPS_LABEL)
                        .text_color(p.ink_3)
                        .child(room.completed_count.to_string()),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ui.show_completed = !this.ui.show_completed;
                    this.refresh(cx);
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
            .child(
                div()
                    .id("task-list")
                    .key_context("TaskList")
                    .track_focus(&self.list_focus)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_action(cx.listener(|this, _: &NextTask, _, cx| this.move_selection(1, cx)))
                    .on_action(
                        cx.listener(|this, _: &PreviousTask, _, cx| this.move_selection(-1, cx)),
                    )
                    // Space completes or reopens the selected task. It's a key handler rather than a
                    // binding so a held key's repeats can be told apart: only the first press counts,
                    // and holding Space never flips a task back and forth.
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                        if e.keystroke.key == "space" && !e.keystroke.modifiers.modified() {
                            cx.stop_propagation();
                            if !e.is_held
                                && let Some(id) = this.ui.task
                                && this.task_is_visible(id)
                            {
                                this.toggle_complete(id, cx);
                            }
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Escape, _, cx| this.select_task(None, cx)))
                    .on_action(cx.listener(|this, _: &EditTask, window, cx| {
                        if this.ui.task.is_some() {
                            let handle = this.edit.title.read(cx).focus_handle.clone();
                            this.edit.title.update(cx, |a, cx| a.select_everything(cx));
                            window.focus(&handle, cx);
                        }
                    }))
                    .on_action(
                        cx.listener(|this, _: &EditDue, window, cx| this.start_due(window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &CyclePriority, _, cx| {
                        if let Some(id) = this.ui.task {
                            this.cycle_priority(id, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &MoveTaskUp, _, cx| this.move_task(-1, cx)))
                    .on_action(cx.listener(|this, _: &MoveTaskDown, _, cx| this.move_task(1, cx)))
                    .children(self.outgoing_list(swap.outgoing, window, cx))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .opacity(swap.incoming)
                            .child(body)
                            // Overlay in the task rows' existing right padding.
                            .when(has_rows, |d| {
                                d.child(self.rows_bar.for_list(&self.rows, &p, self.animate))
                            }),
                    ),
            )
            .children(completed)
    }

    /// The plan button: planning the next day happens in Planned, where tomorrow's tasks are.
    fn plan_button(
        &self,
        weekday: &SharedString,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("plan-next")
            .flex_none()
            .child(kit::button(&self.palette, format!("Plan {weekday}")))
            .when(live && self.list_is_visible(), |el| {
                el.on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    // It sits in the hand-over's first task (or its only line): not there until that enters.
                    if this.list_is_visible() && this.handover_entered(1) {
                        this.select_room(RoomKey::Smart(model::Smart::Planned), cx);
                    }
                }))
            })
    }

    /// The sunrise mark: tomorrow, in the hand-over.
    fn sunrise_mark(&self) -> Div {
        let p = &self.palette;
        div()
            .size(px(desktop::QUICK_ADD + space::S2))
            .flex_none()
            .rounded_full()
            .bg(p.wash)
            .flex()
            .items_center()
            .justify_center()
            .child(kit::icon(
                Icon::Sunrise,
                component::ICON + space::S1,
                p.accent_strong,
            ))
    }

    /// The finished day with nothing due tomorrow: no hand-over, but planning the next day is still
    /// offered, in the hand-over's place.
    fn plan_only(&self, weekday: SharedString, live: bool, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(desktop::LIST_PAD_X))
                    .pt(px(space::S6))
                    .pb(px(space::S2))
                    .text_color(p.ink_3)
                    .style(ty::CAPS_LABEL)
                    .child("When the sun comes up".to_uppercase())
                    .when(live, |d| self.reveal(0, d)),
            )
            .child(
                div()
                    .when(live, |d| self.reveal(1, d))
                    .mx(px(desktop::LIST_PAD_X))
                    .flex()
                    .items_center()
                    .gap(px(space::S4))
                    .child(self.sunrise_mark())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .style(ty::NOTES)
                            .text_color(p.ink_2)
                            .child(format!("Nothing is due on {weekday} yet.")),
                    )
                    .child(self.plan_button(&weekday, live, cx)),
            )
    }

    /// Whether child `i` of tomorrow's hand-over has begun to enter as dusk falls (always, at rest).
    fn handover_entered(&self, i: usize) -> bool {
        self.motion.dusk(self.frame).handover(i).0 > 0.
    }

    fn list_is_visible(&self) -> bool {
        // An event may arrive before the first paint of the new state. A live row has to have
        // been painted, not merely be visible according to a newer wall-clock sample.
        self.motion.navigation.list(self.frame).incoming > 0.
    }

    fn row_is_visible(&self, id: TaskId) -> bool {
        self.list_is_visible()
            && self
                .motion
                .row(id, self.frame)
                .is_none_or(|look| look.height > 0. && look.opacity > 0.)
    }

    fn task_is_visible(&self, id: TaskId) -> bool {
        self.row_is_visible(id) && self.shell.reachable(id, |c| self.handover_entered(c))
    }

    /// The tasks J and K walk, and Space can reach: only ones that can be seen (`Shell::nav_tasks_shown`).
    fn nav_tasks(&self) -> Vec<TaskId> {
        let mut tasks = self.shell.nav_tasks_shown(|c| self.handover_entered(c));
        tasks.retain(|&id| self.row_is_visible(id));
        tasks
    }

    /// Item `i` of tomorrow's hand-over while dusk falls: fading in and rising into place after the
    /// doc's cue, the first few staggered (`motion::DuskLook::handover`). At rest it's left as it is.
    fn reveal<E: Styled>(&self, i: usize, el: E) -> E {
        let (shown, rise) = self.motion.dusk(self.frame).handover(i);
        if shown >= 1. {
            el
        } else {
            el.opacity(shown).relative().top(px(rise))
        }
    }

    /// The finished day hands over to tomorrow: its first task with a sunrise mark, then the rest of
    /// tomorrow and the rest of the week. No congratulations; the scene is the reward.
    fn tomorrow(
        &self,
        next: &Tomorrow,
        outgoing_top: Option<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = &self.palette;
        let t = self.theme;
        let live = outgoing_top.is_none();
        let interactive = live && self.list_is_visible();
        let plan = self.plan_button(&next.weekday, interactive, cx);
        // Every child keeps its height (`flex_none`): the hand-over scrolls rather than squeezing them.
        let section = |label: SharedString| {
            div()
                .flex_none()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6))
                .pb(px(space::S2))
                .text_color(p.ink_3)
                .style(ty::CAPS_LABEL)
                .child(label.to_uppercase())
        };
        // Picking a task opens it in the detail pane, like a row.
        let selected = self.ui.task;
        let pick = |id: TaskId| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select_task(Some(id), cx);
            })
        };
        let compact = |task: &TaskRow| {
            let when = task.due.text().cloned().unwrap_or_default();
            div()
                .id(SharedString::from(format!("next-{}", task.id)))
                .when(selected == Some(task.id), |d| d.bg(p.wash))
                .when(interactive, |el| {
                    el.hover(|s| s.bg(p.hover)).on_click(pick(task.id))
                })
                .flex_none()
                .mx(px(desktop::LIST_PAD_X))
                .h(px(component::ICON_BUTTON + space::S1))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .border_t_1()
                .border_color(p.line)
                .child(kit::checkbox(
                    p,
                    false,
                    component::CHECKBOX - space::S1 / 2.,
                ))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .style(ty::BODY)
                        .text_color(p.ink)
                        .child(task.title.clone()),
                )
                .child(kit::chip(t, &task.list, false))
                .child(div().flex_1())
                .child(div().style(ty::DUE).text_color(p.ink_2).child(when))
        };
        let first = &next.first;
        let mark = self.sunrise_mark();
        let card = div()
            .id("next-first")
            .when(selected == Some(first.id), |d| d.bg(p.wash))
            .when(interactive, |el| el.on_click(pick(first.id)))
            .flex_none()
            .mx(px(desktop::LIST_PAD_X))
            .rounded(px(radius::LG))
            .flex()
            .items_center()
            .gap(px(space::S4))
            .child(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .style(ty::NEXT_TITLE)
                            .text_color(p.ink)
                            .truncate()
                            .child(first.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .style(ty::DUE.with_weight(600))
                                    .text_color(p.accent_strong)
                                    .child(next.first_when.clone()),
                            )
                            .child(kit::chip(t, &first.list, true)),
                    ),
            )
            .child(plan);
        // Its children are in the order `Tomorrow::child` counts them; it scrolls when taller than the pane.
        // While dusk falls they enter in that order too (`reveal`).
        let section_len = |rows: &[TaskRow]| if rows.is_empty() { 0 } else { 1 + rows.len() };
        let also = 2;
        let week = also + section_len(&next.also);
        let more = week + section_len(&next.week);
        let scroll = div()
            .id("handover")
            .size_full()
            .when(live, |el| {
                el.overflow_y_scroll().track_scroll(&self.handover_scroll)
            })
            .when_some(outgoing_top, |el, top| el.relative().top(top))
            .pb(px(space::S6))
            .flex()
            .flex_col()
            .child(section("When the sun comes up".into()).when(live, |el| self.reveal(0, el)))
            .child(card.when(live, |el| self.reveal(1, el)))
            .when(!next.also.is_empty(), |d| {
                d.child(
                    section(format!("Also on {}", next.weekday).into())
                        .when(live, |el| self.reveal(also, el)),
                )
                .children(
                    (next.also.iter().enumerate())
                        .map(|(i, t)| compact(t).when(live, |el| self.reveal(also + 1 + i, el))),
                )
            })
            .when(!next.week.is_empty(), |d| {
                d.child(section("Rest of the week".into()).when(live, |el| self.reveal(week, el)))
                    .children(
                        (next.week.iter().enumerate()).map(|(i, t)| {
                            compact(t).when(live, |el| self.reveal(week + 1 + i, el))
                        }),
                    )
            })
            .when(next.more > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(desktop::LIST_PAD_X))
                        .pt(px(space::S3))
                        .style(ty::COUNT)
                        .text_color(p.ink_3)
                        .child(format!("{} more", next.more))
                        .when(live, |el| self.reveal(more, el)),
                )
            });
        div()
            .id("handover-pane")
            .relative()
            .size_full()
            .child(scroll)
            .when(interactive, |el| {
                el.child(
                    self.handover_bar
                        .overlay(&self.handover_scroll, p, self.animate),
                )
            })
    }

    /// Where the keyboard is, for the status bar's hints.
    fn focus_now(&self, window: &Window, cx: &App) -> hints::Focus {
        use hints::Focus;
        let on = |h: &FocusHandle| h.is_focused(window);
        if on(&self.list_focus) {
            Focus::List
        } else if on(&self.quick_add.read(cx).focus_handle) {
            Focus::QuickAdd
        } else if on(&self.filter.read(cx).focus_handle) {
            Focus::Filter
        } else if on(&self.edit.title.read(cx).focus_handle) {
            Focus::Title
        } else if on(&self.edit.notes.read(cx).focus_handle) {
            Focus::Notes
        } else if on(&self.edit.step.read(cx).focus_handle) {
            Focus::Step
        } else if self
            .edit
            .due
            .as_ref()
            .is_some_and(|(i, _)| on(&i.read(cx).focus_handle))
        {
            Focus::Due
        } else if self
            .inline
            .as_ref()
            .is_some_and(|i| on(&i.input.read(cx).focus_handle))
        {
            Focus::Rename
        } else {
            Focus::Elsewhere
        }
    }

    fn status_bar(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        let open = self.shell.room.summary().2;
        let done = self.shell.room.done_today;
        let keys = hints::key_hints(hints::HintState {
            focus: self.focus_now(window, cx),
            rows: !self.nav_tasks().is_empty(),
            selected: self
                .shell
                .detail
                .as_ref()
                .map(|d| hints::Selected { done: d.done }),
            reorder: matches!(self.ui.room, RoomKey::List(_)),
        });
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(component::CHIP_GAP))
                .child(kit::kbd(p, key))
                .child(what)
        };
        let strong = |s: String| {
            div()
                .text_color(p.ink_2)
                .font_weight(FontWeight::MEDIUM)
                .child(s)
        };
        let error = self.notice.as_ref().is_some_and(|n| n.error);
        let folder = self.notice.as_ref().and_then(|n| n.folder.clone());
        let right: SharedString = self
            .notice
            .as_ref()
            .map(|n| n.text.clone())
            .unwrap_or_else(|| "Ctrl N adds a task".into());
        div()
            .h(px(desktop::STATUSBAR))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S4))
            .border_t_1()
            .border_color(p.line)
            .bg(p.canvas)
            .style(ty::COUNT)
            .text_color(p.ink_3)
            .child(
                div()
                    .flex()
                    .gap(px(space::S4))
                    .child(
                        div()
                            .flex()
                            .gap(px(component::CHIP_GAP))
                            .child(strong(open.to_string()))
                            .child("open"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(component::CHIP_GAP))
                            // Completed since midnight; "Completed n" above holds every finished task.
                            .child(strong(done.to_string()))
                            .child("done today"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S4 + 2.))
                    .style(ty::META)
                    .children(keys.into_iter().map(|(key, what)| hint(key, what))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S2))
                    .when(error, |d| d.text_color(p.danger_ink))
                    .child(right)
                    .when_some(folder, |d, folder| {
                        d.child(
                            div()
                                .id("notice-open-folder")
                                .cursor_pointer()
                                .text_color(p.ink_2)
                                .font_weight(FontWeight::MEDIUM)
                                .underline()
                                .child("Open folder")
                                .on_click(move |_, _, cx| cx.open_with_system(&folder)),
                        )
                    }),
            )
    }
}

/// The clock the motion runs on: real time, or a fixed moment for a screenshot fixture.
#[derive(Clone, Copy, Debug)]
enum Clock {
    /// Time since this instant.
    Live(Instant),
    Frozen(Duration),
}

/// The window's focus starts on the task list, where the single-letter keys work.
impl Focusable for ShellView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.resize(Width::of(f32::from(window.viewport_size().width)));
        self.drive_motion(window, cx);
        let p = self.palette;
        let quick_add = self.quick_add_field(window, cx);
        let detail = self.docked().then(|| self.detail_pane(window, cx));
        let overlay = self.detail_overlay(window, cx);
        div()
            .id("shell")
            .key_context("Shell")
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                let handle = this.filter.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusQuickAdd, window, cx| {
                let handle = this.quick_add.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            // A click that no field takes (the sidebar, the header, a button) leaves the keyboard on the
            // task list, so its keys keep working.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !window.default_prevented() {
                        window.focus(&this.list_focus, cx);
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .style(ty::BODY)
            .text_color(p.ink)
            .child(self.titlebar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_t_1()
                    .border_color(p.line)
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(p.pane)
                            .child(self.header())
                            .child(quick_add)
                            .child(self.list(window, cx))
                            .children(overlay.into_iter().flatten()),
                    )
                    .children(detail),
            )
            .child(self.status_bar(window, cx))
            .children(self.menu(cx))
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures;

    #[test]
    fn each_hand_over_task_maps_to_its_own_child() {
        let shell = fixtures::finished().shell();
        let next = shell.room.handover().expect("hands over");
        // Label, first task, "Also on Sunday", its one task, "Rest of the week", its three tasks.
        let children: Vec<Option<usize>> = next.tasks().map(|t| next.child(t.id)).collect();
        assert_eq!(children, [Some(1), Some(3), Some(5), Some(6), Some(7)]);
        // A task the hand-over doesn't show has no child.
        assert_eq!(
            next.child(todo_core::ids::TaskId::from_uuid(uuid::Uuid::nil())),
            None
        );
    }
}
