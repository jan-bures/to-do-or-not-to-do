//! The main window: titlebar, sidebar, the room pane under its sky header, the detail pane and the status
//! bar. It draws a `model::Shell`, derived from the store's state whenever the data or the window's own
//! state changes, and turns clicks and keys into commands for the store.

mod activity;
mod assistant;
mod attachments;
mod board;
mod bulk;
mod detail;
mod device_pairing;
mod email_drop;
mod menu;
mod panes;
mod popout;
mod quick_pickers;
mod reminders;
mod sidebar;
mod task_drag;
mod timeline;
mod trash;
mod undo;
mod updates;
mod workflow;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AsyncApp, Bounds, Context, DispatchPhase, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, SharedString, Stateful, StrikethroughStyle, UniformListScrollHandle, WeakEntity,
    Window, WindowControlArea, actions, div, prelude::*, px, uniform_list,
};
use todo_client::Connection;
use todo_client::room_header::{StatusCounts, Summary};
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::model::Priority;

use super::input::{InputColors, InputEvent, TextInput};
use super::kit;
use super::landscape::{Blend, HeaderCache, header_landscape, sun_at};
use crate::fixtures::Moment;
use crate::model::activity::FeedInput;
use crate::model::attachments::{Control as FileControl, file_controls};
use crate::model::layout::{DetailPane, Pane, Width};
use crate::model::menus::{Page, TagPage};
use crate::model::motion::{
    DetailMotion, Keep, Motion, Pace, RowLook, Toast, Track, Wake, anchored_scroll, lifts,
    rows_rising_into,
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
use menu::{Menu, MenuKind, TaskPage};
use sidebar::{Inline, InlineTarget};

actions!(
    shell,
    [
        FocusQuickAdd,
        FocusFilter,
        NextTask,
        PreviousTask,
        SelectAllTasks,
        ExtendTasksUp,
        ExtendTasksDown,
        EditTask,
        EditDue,
        CyclePriority,
        PickPriority,
        MoveTaskUp,
        MoveTaskDown,
        MoveTaskTo,
        TaskMenu,
        PickTags,
        PickRepeat,
        PickReminder,
        AddLinkKey,
        UndoTask,
        OpenActivity,
        ToggleAiPanel,
        Escape
    ]
);

/// Key bindings for the main window. The single-letter ones only apply while the task list has focus,
/// never while typing.
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    let list = Some("TaskList");
    let board = Some("TaskBoard");
    vec![
        KeyBinding::new("ctrl-n", FocusQuickAdd, Some("Shell")),
        KeyBinding::new("ctrl-k", FocusFilter, Some("Shell")),
        KeyBinding::new("ctrl-j", ToggleAiPanel, Some("Shell")),
        KeyBinding::new("ctrl-z", UndoTask, Some("Shell")),
        // Activity, from anywhere but a text field (the handler leaves typing alone).
        KeyBinding::new("ctrl-h", OpenActivity, Some("Shell")),
        KeyBinding::new("e", EditTask, list),
        KeyBinding::new("d", EditDue, list),
        KeyBinding::new("p", CyclePriority, list),
        KeyBinding::new("shift-p", PickPriority, list),
        KeyBinding::new("alt-up", MoveTaskUp, list),
        KeyBinding::new("alt-down", MoveTaskDown, list),
        KeyBinding::new("m", MoveTaskTo, list),
        KeyBinding::new("t", PickTags, list),
        KeyBinding::new("r", PickRepeat, list),
        KeyBinding::new("shift-r", PickReminder, list),
        KeyBinding::new("l", AddLinkKey, list),
        KeyBinding::new("shift-f10", TaskMenu, list),
        KeyBinding::new("j", NextTask, list),
        KeyBinding::new("down", NextTask, list),
        KeyBinding::new("k", PreviousTask, list),
        KeyBinding::new("up", PreviousTask, list),
        KeyBinding::new("ctrl-a", SelectAllTasks, list),
        KeyBinding::new("cmd-a", SelectAllTasks, list),
        KeyBinding::new("shift-up", ExtendTasksUp, list),
        KeyBinding::new("shift-down", ExtendTasksDown, list),
        KeyBinding::new("escape", Escape, list),
        KeyBinding::new("ctrl-a", SelectAllTasks, board),
        KeyBinding::new("cmd-a", SelectAllTasks, board),
        KeyBinding::new("shift-up", ExtendTasksUp, board),
        KeyBinding::new("shift-down", ExtendTasksDown, board),
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
    RestartToUpdate,
    PopOut(model::popouts::Room),
}

#[derive(Clone, Copy)]
struct ToggleMotion {
    start: Duration,
    span: Duration,
    from: f32,
    on: bool,
    reduced: bool,
}

impl ToggleMotion {
    fn at(self, now: Duration) -> (f32, f32) {
        use todo_design::motion as m;
        let t = (now.saturating_sub(self.start).as_secs_f32() / self.span.as_secs_f32()).min(1.);
        let fill = self.from + (f32::from(self.on) - self.from) * m::STANDARD.ease(t);
        let scale = if self.on && !self.reduced {
            m::TOGGLE_FILL_FROM_SCALE + (1. - m::TOGGLE_FILL_FROM_SCALE) * m::POP.ease(t)
        } else {
            1.
        };
        (fill, scale)
    }
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
        rows: Vec<OutgoingRow>,
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
    Trash {
        rows: Vec<todo_client::trash::Row>,
        cut: f32,
    },
    /// The Activity room's changes, as painted, at the feed's scroll.
    Activity {
        days: Vec<model::activity::ActivityDay>,
        top: gpui::Pixels,
    },
}

#[derive(Debug)]
struct OutgoingRow {
    task: TaskRow,
    selected: bool,
    look: Option<RowLook>,
    lift: f32,
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
    my_day_progress: model::MyDayProgress,
    theme: Theme,
    palette: Palette,
    painted_hue: Hue,
    landscape: HeaderCache,
    quick_add: Entity<TextInput>,
    quick_draft: quick_add::Draft,
    quick_focus: quick_pickers::Controls,
    workflow: workflow::Controls,
    board: board::BoardUi,
    timeline: timeline::TimelineUi,
    list_focus: FocusHandle,
    multi: todo_client::selection::Selection,
    task_drag: Option<task_drag::Gesture>,
    task_settle: Option<task_drag::Settle>,
    bulk_focus: [FocusHandle; 8],
    rows: UniformListScrollHandle,
    trash_selected: Option<todo_core::ids::EntityId>,
    /// The finished day's hand-over, which scrolls when it's taller than the pane.
    handover_scroll: ScrollHandle,
    handover_bar: kit::Scrollbar,
    rows_bar: kit::Scrollbar,
    sidebar_scroll: ScrollHandle,
    sidebar_bar: kit::Scrollbar,
    splitters: panes::Splitters,
    detail_scroll: ScrollHandle,
    attachment_ui: attachments::AttachmentUi,
    detail_bar: kit::Scrollbar,
    caption_preview: kit::CaptionPreview,
    compact: Option<popout::Compact>,
    menu: Option<Menu>,
    /// The menu the current mouse press closed, if it closed one: the click that press ends on that
    /// menu's own button leaves it closed (`toggle_menu`). Every press in the window is counted.
    menu_press: menu::PressGuard,
    /// The open menu's, while it has the keyboard.
    menu_focus: FocusHandle,
    /// A long menu's scroll (the list picker).
    menu_scroll: ScrollHandle,
    /// The open menu was dropped because its subject went away; the keyboard goes back to the list.
    menu_dropped: bool,
    /// Where each task row in the room was last painted: a task's menu opened from the keyboard hangs
    /// under its row.
    row_bounds: Rc<RefCell<HashMap<TaskId, Bounds<Pixels>>>>,
    /// Where each list and collection row in the sidebar was last painted: a screenshot fixture opens
    /// its context menu there, as a right-click would.
    nav_bounds: Rc<RefCell<HashMap<RoomKey, Bounds<Pixels>>>>,
    /// Where controls inside a scrolled pane were last painted, so landing on them scrolls them into
    /// view (`scroll_into_view`).
    probes: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    /// The footer's New button, which Tab reaches.
    new_focus: FocusHandle,
    /// Where a screenshot fixture that typed into a field puts the keyboard.
    preview_focus: Option<FocusHandle>,
    /// The sidebar footer's "N couldn't sync", a button while something couldn't sync.
    sync_focus: FocusHandle,
    /// The room header's "more" button and the sidebar's account gear, which Tab reaches.
    room_menu_focus: FocusHandle,
    repeat_focus: FocusHandle,
    due_focus: FocusHandle,
    detail_my_day_focus: FocusHandle,
    detail_important_focus: FocusHandle,
    reminder_focus: FocusHandle,
    account_focus: FocusHandle,
    updates: Option<crate::updates::Controller>,
    update_focus: FocusHandle,
    dismiss_update_focus: FocusHandle,
    pairing_panel: Option<Entity<device_pairing::Panel>>,
    ai: assistant::Ai,
    invite_panel: Option<Entity<device_pairing::Panel>>,
    inline: Option<Inline>,
    /// The filter typed into the title bar.
    filter: Entity<TextInput>,
    edit: Editors,
    /// The completion choreography and the sun's travel (`model::motion`), sampled at `clock`.
    motion: Motion,
    toast: Toast<SharedString>,
    undo_focus: FocusHandle,
    undo_ring: Track,
    toast_hover: bool,
    last_undo: Option<undo::Action>,
    toast_without_undo: bool,
    email_importing: bool,
    preview_email_drop: bool,
    email_drop_fade: model::motion::Track,
    pending_undo: Vec<undo::Pending>,
    clock: Clock,
    /// The time this frame draws (`clock` when the render started), so every row samples the same moment.
    frame: Duration,
    task_toggles: HashMap<(TaskId, Icon), ToggleMotion>,
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
    /// The Activity room's feed, selection and undos (`model::activity`), while it shows.
    activity: model::activity::ActivityState,
    activity_scroll: ScrollHandle,
    activity_bar: kit::Scrollbar,
    /// Where each change was last painted, so the keyboard's selection scrolls into view.
    activity_rows: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    /// The room's Clear filter, Refresh activity and the button at the end of the feed (Tab stops).
    activity_filter_focus: FocusHandle,
    activity_refresh_focus: FocusHandle,
    activity_more_focus: FocusHandle,
    /// The live session's reminder toasts (`start_reminders`); none for a fixture.
    alerts: Option<reminders::Alerts>,
    /// Closing a window cancels its watchers, even if no further sync change comes in.
    watchers: Vec<gpui::Task<()>>,
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
        let (state, now) = (store.view(), store.now());
        let mut my_day_progress = model::MyDayProgress::default();
        my_day_progress.update(&state, now.today());
        let shell = model::derive(&state, &ui, &store.sync(), &now);
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
                    let mut draft = this.quick_draft.clone();
                    if let Some(status) = this
                        .board
                        .composer
                        .and_then(workflow::open_stage)
                        .filter(|_| this.on_board())
                    {
                        draft.pick(
                            quick_add::Pick::Status(Some(status)),
                            &this.store.now().eval(),
                            this.quick_features(),
                        );
                    }
                    if this.add_draft(&draft, this.ui.room, cx) {
                        this.quick_draft = quick_add::Draft::default();
                        input.update(cx, |i, cx| i.reset(window, cx));
                        cx.notify();
                    }
                }
                InputEvent::Cancel => {
                    if this.on_board() {
                        this.board.composer = None;
                        this.quick_draft = quick_add::Draft::default();
                        input.update(cx, |i, cx| i.reset(window, cx));
                        cx.notify();
                    }
                    if this.ui.compact {
                        this.close_popout(window);
                    } else {
                        window.focus(&this.list_focus, cx);
                    }
                }
                InputEvent::Changed => {
                    let features = this.quick_features();
                    this.quick_draft.edit_with_features(
                        input.read(cx).content.to_string(),
                        &this.store.now().eval(),
                        features,
                    );
                    cx.notify();
                }
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
        let ai = assistant::Ai::new(&store, &palette, window, cx);
        let mut this = Self {
            store,
            ui,
            shell,
            my_day_progress,
            theme,
            palette,
            painted_hue,
            landscape: HeaderCache::default(),
            quick_draft: quick_add::Draft {
                text: quick_add.read(cx).content.to_string(),
                ..Default::default()
            },
            quick_focus: quick_pickers::Controls::new(quick_add.read(cx).focus_handle.clone(), cx),
            quick_add,
            workflow: workflow::Controls::new(cx),
            board: board::BoardUi::default(),
            timeline: timeline::TimelineUi::new(cx),
            list_focus: cx.focus_handle(),
            rows: UniformListScrollHandle::new(),
            multi: todo_client::selection::Selection::default(),
            task_drag: None,
            task_settle: None,
            bulk_focus: std::array::from_fn(|_| cx.focus_handle()),
            trash_selected: None,
            handover_scroll: ScrollHandle::new(),
            handover_bar: kit::Scrollbar::default(),
            rows_bar: kit::Scrollbar::default(),
            sidebar_scroll: ScrollHandle::new(),
            sidebar_bar: kit::Scrollbar::default(),
            splitters: panes::Splitters::new(window, cx),
            detail_scroll: ScrollHandle::new(),
            attachment_ui: attachments::AttachmentUi::new(cx),
            detail_bar: kit::Scrollbar::default(),
            caption_preview: kit::CaptionPreview::Live,
            compact: None,
            menu: None,
            menu_press: menu::PressGuard::default(),
            menu_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            menu_dropped: false,
            row_bounds: Rc::default(),
            nav_bounds: Rc::default(),
            probes: Rc::default(),
            new_focus: cx.focus_handle(),
            preview_focus: None,
            sync_focus: cx.focus_handle(),
            room_menu_focus: cx.focus_handle(),
            repeat_focus: cx.focus_handle(),
            due_focus: cx.focus_handle(),
            detail_my_day_focus: cx.focus_handle(),
            detail_important_focus: cx.focus_handle(),
            reminder_focus: cx.focus_handle(),
            account_focus: cx.focus_handle(),
            updates: None,
            update_focus: cx.focus_handle(),
            dismiss_update_focus: cx.focus_handle(),
            pairing_panel: None,
            ai,
            invite_panel: None,
            inline: None,
            filter,
            edit,
            motion: Motion::default(),
            toast: Toast::default(),
            undo_focus: cx.focus_handle(),
            undo_ring: Track::rest(0.),
            toast_hover: false,
            last_undo: None,
            toast_without_undo: false,
            email_importing: false,
            preview_email_drop: false,
            email_drop_fade: model::motion::Track::rest(0.),
            pending_undo: Vec::new(),
            clock: Clock::Live(Instant::now()),
            frame: Duration::ZERO,
            task_toggles: HashMap::new(),
            row_lifts: Vec::new(),
            alarm: None,
            notice: None,
            width: Width::of(f32::from(window.viewport_size().width)),
            outgoing: None,
            closing: None,
            closing_groups: HashMap::new(),
            animate: true,
            activity: model::activity::ActivityState::default(),
            activity_scroll: ScrollHandle::new(),
            activity_bar: kit::Scrollbar::default(),
            activity_rows: Rc::default(),
            activity_filter_focus: cx.focus_handle(),
            activity_refresh_focus: cx.focus_handle(),
            activity_more_focus: cx.focus_handle(),
            alerts: None,
            watchers: Vec::new(),
        };
        cx.on_focus(&this.undo_focus, window, |_, _, cx| cx.notify())
            .detach();
        cx.on_blur(&this.undo_focus, window, |_, _, cx| cx.notify())
            .detach();
        // A menu never stays open once the keyboard has gone elsewhere.
        cx.on_blur(&this.menu_focus, window, |this, window, cx| {
            this.menu_lost_focus(window, cx);
        })
        .detach();
        if let Some(id) = this.ui.task {
            this.aim_board_cursor(id);
        }
        this.load_detail(cx);
        this.settle_detail();
        let _ = this.follow_shell();
        if let Some(agent) = this.activity_room() {
            this.open_activity(agent, cx);
        }
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
        let changes = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                changes.next().await;
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let clock = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        self.watchers = vec![changes, clock];
    }

    /// Re-derives the shell from the store and the UI state.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_with_collapse_change(false, cx);
    }

    fn refresh_with_collapse_change(&mut self, collapsed_changed: bool, cx: &mut Context<Self>) {
        self.refresh_ai(cx);
        self.drain_reminder_actions();
        let state = self.undo_state();
        // Capture the painted rows before releasing the old room's holds and deriving new counts.
        let room = self.ui.room.resolved(&state);
        if self.ui.compact {
            let available = room == self.ui.room;
            self.popout_available(available, cx);
            if !available {
                // A fixed room never turns into My Day during catch-up or after removal.
                self.menu = None;
                self.menu_dropped = true;
                return;
            }
        }
        let room_changed = room != self.shell.room.key;
        let old_view = self.view();
        let outgoing_body = (room_changed && self.animate).then(|| self.capture_body(0.));
        if self
            .last_undo
            .as_ref()
            .is_some_and(|undo| !undo.is_in(room))
        {
            self.clear_undo();
        }
        if room_changed {
            self.board.reset_room();
            self.timeline.reset_view();
            self.multi.clear();
            self.motion.settle_all();
            self.row_lifts.clear();
            self.row_bounds.borrow_mut().clear();
        }
        let stale_menu = self.menu.as_ref().is_some_and(|m| match m.kind {
            MenuKind::Room { .. }
            | MenuKind::Task { .. }
            | MenuKind::BulkMove
            | MenuKind::BulkStatus
            | MenuKind::QuickPriority
            | MenuKind::Status { .. } => room_changed,
            MenuKind::Date(c) => {
                room_changed
                    || c.task.is_some_and(|task| {
                        if let Some(dates) = c.timeline {
                            !self.on_timeline()
                                || state.tasks.get(&task).is_none_or(|t| {
                                    dates != todo_client::intent::workflow::Dates::of(t)
                                        || !todo_client::timeline::includes(
                                            &state,
                                            model::shared_key(room),
                                            t,
                                        )
                                })
                        } else {
                            self.ui.task != Some(task)
                        }
                    })
            }
            MenuKind::Priority { task }
            | MenuKind::Tags { task, .. }
            | MenuKind::Repeat { task, .. }
            | MenuKind::Reminder { task, .. } => self.ui.task != Some(task),
            _ => false,
        });
        if stale_menu {
            self.menu = None;
            self.menu_dropped = true;
        }
        self.follow_data(&state);
        let session = self.store.session();
        // Prune only once the stream is live and caught up: during catch-up after a new or rebuilt
        // cache, a collection from a later page isn't in the replica yet. Stale IDs are harmless
        // meanwhile, since deriving the sidebar only asks whether an ID is collapsed.
        let can_prune =
            session.is_none_or(|s| s.client().status().connection == Connection::Online);
        let pruned = can_prune && self.ui.prune_collapsed_collections(&state);
        if collapsed_changed || pruned {
            self.save_ui_preferences();
        }
        let now = self.store.now();
        self.my_day_progress.update(&state, now.today());
        let shell = model::derive(&state, &self.ui, &self.store.sync(), &now);
        // A room that disappeared (deleted elsewhere) falls back to My Day; keep the UI state honest.
        self.ui.room = shell.room.key;
        if shell.detail.is_none() {
            self.ui.task = None;
        }
        if shell.room.key != self.shell.room.key {
            self.start_room_switch(&shell, outgoing_body);
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
        self.attachment_ui.uploads.select(new_task);
        if let (Some(task), Some(session)) = (new_task, self.store.session()) {
            session.client().with_confirmed(|s| {
                self.attachment_ui
                    .uploads
                    .observe(task, |id| s.attachments.contains_key(&id));
            });
        }
        if old_task != new_task {
            if new_task.is_none() {
                self.closing = self.shell.detail.take();
            } else {
                self.closing = None;
                self.detail_bar.reset_to_top(&self.detail_scroll);
            }
        }
        if self.animate && !self.ui.compact {
            self.aim_sidebar(&shell);
        }
        self.trash_selected = self.trash_selected.filter(|id| {
            shell
                .room
                .trash
                .iter()
                .any(|r| r.id == *id && r.blocked.is_none())
        });
        self.shell = shell;
        if !room_changed && old_view != self.view() {
            self.board.reset_view();
            self.timeline.reset_view();
            self.timeline.cursor = self.ui.task;
            self.board.start_swap(self.now(), self.pace(), self.animate);
            if let Some(id) = self.ui.task {
                self.aim_board_cursor(id);
            }
        }
        if self.multi.cursor().is_some() {
            self.multi.retain_visible(&self.nav_tasks());
        }
        if self.animate {
            self.aim_detail();
        } else {
            self.settle_detail();
        }
        match self.activity_room() {
            Some(agent) if room_changed => self.open_activity(agent, cx),
            _ => self.follow_activity(cx),
        }
        if self.follow_shell() {
            // A held task the list no longer shows was let go: derive again without it. That pass lets
            // nothing else go, since only shown rows are left.
            return self.refresh_with_collapse_change(false, cx);
        }
        self.load_detail(cx);
        // Every change to the data (a sync, an edit, the half-minute tick) brings the reminder toasts
        // in line with it.
        self.reconcile_reminders(cx);
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
        self.ui
            .held_updates
            .retain(|id, _| self.ui.holding.contains(id));
        let reopening: Vec<TaskId> = self.motion.reopening().collect();
        self.ui.reopening.retain(|id, _| reopening.contains(id));
        let leaving: Vec<TaskId> = self.motion.leaving().collect();
        self.ui.leaving.retain(|id, _| leaving.contains(id));
    }

    /// After deriving: the sun heads for the new count, and rows the
    /// list doesn't show stop moving (see `Motion::retain_shown`). Returns whether that let a held task
    /// go, so the shell must be derived again.
    fn follow_shell(&mut self) -> bool {
        let now = self.now();
        let room = &self.shell.room;
        let pace = self.pace();
        self.motion.aim_header(
            room.key,
            sun_at(room.done, room.total),
            room.finished_day() && !self.ui.compact,
            now,
            pace,
        );
        let shown: std::collections::HashSet<TaskId> = if self.on_timeline() {
            self.timeline_ids().into_iter().collect()
        } else if self.on_board() {
            self.shell
                .room
                .board
                .iter()
                .flat_map(|b| b.columns.iter().flatten().map(|r| r.id))
                .collect()
        } else {
            self.shell.shown_tasks().collect()
        };
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
        self.toast.advance(now, self.pace());
        let undo_focused = self.undo_focus.is_focused(window);
        if self.toast.look(now).is_some_and(|t| t.active) {
            self.toast.pause(self.toast_hover || undo_focused, now);
        } else {
            self.toast_hover = false;
            if undo_focused {
                window.focus(&self.list_focus, cx);
            }
        }
        let show_ring = undo_focused && self.toast.look(now).is_some_and(|t| t.active);
        self.undo_ring.aim(
            f32::from(show_ring),
            now,
            Duration::from_millis(u64::from(theme::motion::HOVER)),
            if show_ring {
                theme::motion::STANDARD
            } else {
                theme::motion::EXIT
            },
        );
        self.activity.motion.prune(now);
        let ring =
            if self.undo_ring.moving(now) || self.settle_menu() || self.activity.motion.moving(now)
            {
                Wake::Frame
            } else {
                Wake::Idle
            };
        match self.motion.wake(now).or(self.toast.wake(now)).or(ring) {
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
                let done = self.shell.room.done.saturating_sub(1);
                let total = self.shell.room.total;
                self.motion
                    .place_sun(room, sun_at(done, total).unwrap_or(0.), Duration::ZERO);
                self.motion
                    .aim_dusk(room, false, Duration::ZERO, Pace::FULL);
                at
            }
            Moment::UndoToast { task, at } => {
                if let Some(task) = self.store.view().tasks.get(&task) {
                    self.toast.show(
                        undo::EditKind::Complete.label(&task.title),
                        Duration::ZERO,
                        Pace::FULL,
                    );
                }
                if self.width == Width::Narrow {
                    let scroll = self.detail_scroll.clone();
                    // Once the viewport and content bounds exist, show the attachments at the end.
                    window.on_next_frame(move |_, _| scroll.scroll_to_bottom());
                }
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
                let body = self.capture_body(f32::from(window.viewport_size().height));
                self.begin_navigation(&next, body);
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

    /// The keyboard never stays on something that's gone: a menu that closed or has nothing left, or the
    /// room's "more" button in a room without a menu. It goes back to the list.
    fn keep_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A menu whose subject went away (a list or task deleted on another device) has nothing left to
        // offer: it closes at once, and the keyboard goes back to the list, so Escape and Tab still work.
        if self.menu.as_ref().is_some_and(|m| match m.kind {
            MenuKind::Date(c) => c.task.is_some_and(|task| {
                if c.timeline.is_some() {
                    !self.on_timeline() || !self.timeline_visible(task)
                } else {
                    self.repeat_target() != Some(task) || c.starts && !self.task_has_start(task)
                }
            }),
            MenuKind::Status {
                task: Some(task),
                at_row,
            } => {
                self.task_stage(task).is_none()
                    || if at_row {
                        !self.task_is_visible(task)
                    } else {
                        self.repeat_target() != Some(task)
                    }
            }
            _ => self.menu_entries(m.kind).is_empty(),
        }) {
            self.menu = None;
            self.menu_dropped = true;
        }
        let dropped = std::mem::take(&mut self.menu_dropped);
        let menu_open = self
            .menu
            .as_ref()
            .is_some_and(|m| m.overlay.look(self.frame).open);
        let menu_focused = self.menu_focus.is_focused(window);
        if menu::keyboard_back_to_list(dropped, menu_focused, window.focused(cx).is_none())
            || (menu_focused && !menu_open)
            || [
                hints::Stop::Status,
                hints::Stop::Starts,
                hints::Stop::View(0),
                hints::Stop::View(1),
                hints::Stop::View(2),
            ]
            .into_iter()
            .any(|stop| self.stop_handle(stop).is_focused(window) && !self.has_stop(stop))
            || (self.due_focus.is_focused(window) && !self.has_stop(hints::Stop::Due))
            || (self.repeat_focus.is_focused(window) && !self.has_stop(hints::Stop::Repeat))
            || (self.reminder_focus.is_focused(window) && !self.has_stop(hints::Stop::Reminder))
            || (self.detail_my_day_focus.is_focused(window)
                && !self.has_stop(hints::Stop::DetailMyDay))
            || (self.detail_important_focus.is_focused(window)
                && !self.has_stop(hints::Stop::DetailImportant))
            || (self.room_menu_focus.is_focused(window) && !self.has_room_menu())
            || [
                hints::Stop::ActivityFilter,
                hints::Stop::ActivityRefresh,
                hints::Stop::ActivityMore,
            ]
            .into_iter()
            .chain(hints::QUICK_ADD_STOPS)
            .chain((0..timeline::CONTROL_COUNT).map(hints::Stop::Timeline))
            .any(|stop| self.stop_handle(stop).is_focused(window) && !self.has_stop(stop))
            || self
                .bulk_focus
                .iter()
                .enumerate()
                .any(|(i, focus)| focus.is_focused(window) && !self.bulk_stop(i))
            || (self.sync_focus.is_focused(window) && self.store.sync().failed == 0)
        {
            window.focus(&self.list_focus, cx);
        }
    }

    /// Opens a menu (or an editor) for a screenshot fixture, at rest. A task's context menu opens once
    /// its row has been laid out, under the row as from the keyboard (`task-context-menu` on the row
    /// after the selected one, `move-to-picker` and `task-links` on the selected one); a list's or a
    /// collection's context menu opens at its sidebar row, as a right-click leaves it. The others hang
    /// from their buttons or rows.
    pub fn preview_menu(&mut self, fixture: &str, window: &mut Window, cx: &mut Context<Self>) {
        match fixture {
            "timeline" | "timeline-tray" | "timeline-month" | "timeline-planned" => {
                if fixture == "timeline-planned" {
                    self.board.view_override = Some(todo_core::model::ListView::Timeline);
                }
                if matches!(fixture, "timeline" | "timeline-tray") {
                    self.timeline.cursor = self
                        .shell
                        .room
                        .timeline
                        .as_ref()
                        .and_then(|t| t.lanes.get(3))
                        .map(|(l, _)| l.task);
                }
                return;
            }
            "board" => {
                if let Some(id) = self
                    .shell
                    .room
                    .board
                    .as_ref()
                    .and_then(|b| b.columns[2].first())
                    .map(|r| r.id)
                {
                    self.aim_board_cursor(id);
                }
                self.toast.show(
                    "Moved “Strip wallpaper…” to In progress".into(),
                    Duration::ZERO,
                    Pace::FULL,
                );
                self.clock = Clock::Frozen(Duration::from_millis(
                    u64::from(theme::motion::UNDO_WINDOW) / 3,
                ));
                return;
            }
            "list-status" => {
                let today = self.store.now().today();
                self.quick_draft.status = Some(todo_core::model::WorkflowStatus::InProgress);
                self.quick_draft.start_date = todo_core::time::add_days(today, 5);
                self.quick_draft.due = todo_core::time::add_days(today, 7)
                    .map_or(quick_add::DueChoice::Default, quick_add::DueChoice::Date);
                return;
            }
            "list-settings" => {
                self.board.view_override = Some(todo_core::model::ListView::List);
                self.settle_detail();
            }
            _ => {}
        }
        if matches!(fixture, "drag-reorder" | "drag-destination") {
            self.preview_task_drag(fixture, window, cx);
            return;
        }
        if fixture == "multi-select" {
            self.change_selection(
                |selection, rows, _| {
                    if let Some(last) = rows.get(2) {
                        selection.range(rows, *last, rows.first().copied());
                    }
                },
                cx,
            );
            return;
        }
        let pairing = match fixture {
            "device-settings" => Some(device_pairing::Mode::Settings),
            "device-invite" => Some(device_pairing::Mode::Invite),
            "device-approve" => Some(device_pairing::Mode::Approve),
            _ => None,
        };
        if let Some(mode) = pairing {
            self.open_pairing_panel(mode, window, cx);
            return;
        }
        if fixture.starts_with("attachments-")
            && let Some(task) = self.ui.task
        {
            match fixture {
                "attachments-uploading" => {
                    self.attachment_ui.uploads.start(task, false);
                }
                "attachments-error" => self.attachment_ui.uploads.finish(
                    task,
                    Some(task),
                    Some(
                        todo_client::failure::attachment_upload(
                            Some(413),
                            Some(todo_core::MAX_ATTACHMENT_BYTES),
                        )
                        .into_owned(),
                    ),
                    Vec::new(),
                ),
                _ => {}
            }
            let scroll = self.detail_scroll.clone();
            window.on_next_frame(move |_, _| scroll.scroll_to_bottom());
            return;
        }
        // An undo asked for from the keyboard: the change selected, then Enter.
        let change = match fixture {
            "activity-pending" => Some(crate::fixtures::PENDING_ACTIVITY_SEQ),
            "activity-refused" => Some(crate::fixtures::REFUSED_ACTIVITY_SEQ),
            _ => None,
        };
        if let Some(seq) = change {
            self.select_change(Some(seq), cx);
            self.undo_change(seq, cx);
            return;
        }
        let task_page = match fixture {
            "task-context-menu" => Some(TaskPage::Main),
            "move-to-picker" => Some(TaskPage::Move),
            "merge-task-picker" => Some(TaskPage::PickMerge),
            "merge-task-confirm" => self.ui.task.and_then(|source| {
                todo_client::intent::merge::candidates(&self.store.view(), source, "").and_then(
                    |choices| {
                        choices
                            .into_iter()
                            .next()
                            .map(|c| TaskPage::ConfirmMerge(c.id))
                    },
                )
            }),
            "task-links" => Some(TaskPage::Links),
            _ => None,
        };
        if let Some(page) = task_page {
            let rows: Vec<TaskId> = self.shell.rows().map(|r| r.id).collect();
            let at = self.shell.selected_task.unwrap_or(0);
            let id = match page {
                TaskPage::Main => rows.get(at + 1).or(rows.first()),
                _ => self.ui.task.as_ref(),
            };
            let Some(&id) = id else {
                return;
            };
            let view = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                view.update(cx, |this, cx| {
                    let at = this.row_anchor(id, window);
                    let kind = MenuKind::Task { id, page };
                    if page == TaskPage::Main {
                        // As a right-click leaves it: at the row, nothing highlighted.
                        this.open_menu(kind, Some(at), None, window, cx);
                    } else {
                        // As the keyboard leaves it: on the task's own list, or the first link item.
                        this.open_menu(kind, Some(at), Some(hints::Stop::List), window, cx);
                        this.highlight_first();
                    }
                })
                .ok();
            });
            return;
        }
        let state = self.store.view();
        let list_named = |name: &str| state.lists.values().find(|l| l.name == name).map(|l| l.id);
        let collection_named =
            |name: &str| (state.collections.values().find(|c| c.name == name)).map(|c| c.id);
        // A list's or collection's context menu, at its sidebar row once it has been laid out.
        let nav = match fixture {
            "collection-menu" => collection_named("Work").map(|id| {
                (
                    RoomKey::Collection(id),
                    MenuKind::Collection {
                        id,
                        page: Page::Main,
                    },
                )
            }),
            "list-context-menu" => list_named("Errands").map(|id| {
                (
                    RoomKey::List(id),
                    MenuKind::List {
                        id,
                        page: Page::Main,
                    },
                )
            }),
            _ => None,
        };
        if let Some((key, kind)) = nav {
            let view = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                view.update(cx, |this, cx| {
                    let bounds = this.nav_bounds.borrow().get(&key).copied();
                    if let Some(b) = bounds {
                        let at = gpui::point(
                            b.origin.x + px(space::S10 * 2.),
                            b.origin.y + b.size.height - px(space::S1),
                        );
                        this.open_menu(kind, Some(at), None, window, cx);
                    }
                })
                .ok();
            });
            return;
        }
        let task = self.ui.task;
        // The first of the open task's tags, by name.
        let tag = self
            .shell
            .detail
            .as_ref()
            .and_then(|d| d.tags.first())
            .map(|t| t.id);
        match fixture {
            "short-sidebar-new-list" => {
                self.new_list(None, window, cx);
                return;
            }
            "short-sidebar-new-collection" => {
                self.new_collection(window, cx);
                return;
            }
            "new-collection" => {
                if let RoomKey::Collection(id) = self.ui.room {
                    self.start_inline(
                        InlineTarget::Collection(id),
                        "New collection".into(),
                        window,
                        cx,
                    );
                }
                return;
            }
            "repeat-focus" => {
                self.preview_focus = Some(self.repeat_focus.clone());
                return;
            }
            "reminder-focus" => {
                self.preview_focus = Some(self.reminder_focus.clone());
                return;
            }
            // Tab onto New in a long sidebar: pinned under the scrolled rooms, with its ring.
            "tab-new" => {
                self.preview_focus = Some(self.new_focus.clone());
                return;
            }
            "tag-rename" => {
                if let Some(chip) = self.shell.detail.as_ref().and_then(|d| d.tags.first()) {
                    let (id, name) = (chip.id, chip.name.clone());
                    self.start_inline(InlineTarget::Tag(id), name, window, cx);
                }
                return;
            }
            "add-link" => {
                self.edit.link.update(cx, |i, cx| {
                    i.set_text("docs.example.com/q4/brief".to_string(), cx);
                });
                self.preview_focus = Some(self.edit.link.read(cx).focus_handle.clone());
                // As L leaves it: the field scrolled into view once the pane is laid out.
                let view = cx.entity().downgrade();
                window.on_next_frame(move |_, cx| {
                    view.update(cx, |this, cx| {
                        if this.scroll_into_view(&this.detail_scroll, "link-field") {
                            cx.notify();
                        }
                    })
                    .ok();
                });
                return;
            }
            _ => {}
        }
        let tags = |page| task.map(|task| MenuKind::Tags { task, page });
        let kind = match fixture {
            "room-menu" | "activity-menu" => Some(MenuKind::Room { page: Page::Main }),
            "list-settings" => Some(MenuKind::Room {
                page: Page::Settings,
            }),
            "account-menu" => Some(MenuKind::Account { confirm: false }),
            "sidebar-new-menu" | "tab-new-collection" => Some(MenuKind::New),
            "sync-failures" => Some(MenuKind::SyncFailures),
            "priority-menu" => task.map(|task| MenuKind::Priority { task }),
            "quick-add-calendar" => Some(MenuKind::Date(self.quick_calendar())),
            "detail-due-calendar" => task.map(|task| self.detail_calendar(task)),
            "quick-add-priority" => Some(MenuKind::QuickPriority),
            "repeat-menu" => task.map(|task| MenuKind::Repeat { task, draft: None }),
            "repeat-custom" => task.and_then(|task| {
                self.store
                    .view()
                    .tasks
                    .get(&task)
                    .map(|t| MenuKind::Repeat {
                        task,
                        draft: Some(crate::model::menus::repeat::Draft::new(
                            t,
                            self.store.now().today(),
                        )),
                    })
            }),
            "reminder-menu" => task.map(|task| self.reminder_menu(task, None)),
            "reminder-custom" => task.map(|task| {
                let now = self.store.now();
                let state = self.store.view();
                let seen = crate::model::commands::reminder::Seen::of(&state, task, &now.eval());
                let due = state.tasks.get(&task).and_then(|t| t.due);
                let mut draft =
                    crate::model::menus::reminder::Draft::new(seen.map(|s| s.at), due, &now);
                // As typing leaves it: tomorrow, 08:30 typed.
                draft.edit(crate::model::menus::reminder::Action::Day(1), now.today());
                for key in ["0", "8", "3", "0"] {
                    draft.type_key(key);
                }
                self.reminder_menu(task, Some(draft))
            }),
            "delete-tag-confirm" => tag.and_then(|tag| tags(TagPage::Confirm(tag))),
            "delete-list-confirm" | "delete-collection-confirm" => Some(MenuKind::Room {
                page: Page::Confirm,
            }),
            "move-list-picker" => Some(MenuKind::Room { page: Page::Move }),
            "list-colour" => Some(MenuKind::Room { page: Page::Colour }),
            "tag-picker" | "tag-create" => tags(TagPage::Pick),
            "tag-edit" => tag.and_then(|tag| tags(TagPage::Edit(tag))),
            "tag-colour" => tag.and_then(|tag| tags(TagPage::Colour(tag))),
            _ => None,
        };
        let Some(kind) = kind else {
            return;
        };
        self.open_menu(kind, None, None, window, cx);
        if fixture == "tag-create"
            && let Some(menu) = self.menu.as_mut()
        {
            menu.filter = "receipts".into();
        }
        // Show Collection inside New rather than as its old separate Tab stop. Confirmations start on
        // Cancel; other pickers or pages start on their first choice.
        if fixture == "tab-new-collection" {
            self.highlight(Some(1));
        } else if !matches!(
            fixture,
            "room-menu"
                | "list-settings"
                | "account-menu"
                | "sync-failures"
                | "priority-menu"
                | "reminder-custom"
        ) {
            self.highlight_first();
        }
    }

    /// Shift+P: the priority picker, from the keyboard, on the detail pane's Priority row. The keyboard
    /// starts on the current level and comes back to the list once one is chosen.
    fn pick_priority(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.ui.task else {
            return;
        };
        if self.detail_placement() == DetailPane::Hidden || !self.task_is_visible(task) {
            return;
        }
        self.open_menu(
            MenuKind::Priority { task },
            None,
            Some(hints::Stop::List),
            window,
            cx,
        );
        self.highlight_first();
    }

    /// The reminder picker for `task`, made on the reminder its row shows now.
    fn reminder_menu(
        &self,
        task: TaskId,
        draft: Option<model::menus::reminder::Draft>,
    ) -> MenuKind {
        let now = self.store.now();
        let seen = model::commands::reminder::Seen::of(&self.store.view(), task, &now.eval());
        MenuKind::Reminder {
            task,
            seen,
            draft,
            presets: model::menus::reminder::Presets::new(&now),
        }
    }

    /// Shift+R (or Enter on the row): the reminder picker on the detail pane's Reminder row, the
    /// keyboard on the current choice.
    fn pick_reminder(&mut self, trigger: hints::Stop, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.repeat_target() else {
            return;
        };
        self.scroll_into_view(&self.detail_scroll, "reminder-row");
        let kind = self.reminder_menu(task, None);
        self.open_menu(kind, None, Some(trigger), window, cx);
        self.highlight_first();
    }

    /// T: the tag picker, from the keyboard, on the detail pane's Tags row. The keyboard starts on the
    /// first tag and comes back to the list when the picker closes.
    fn pick_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.ui.task else {
            return;
        };
        if self.detail_placement() == DetailPane::Hidden || !self.task_is_visible(task) {
            return;
        }
        let kind = MenuKind::Tags {
            task,
            page: TagPage::Pick,
        };
        self.open_menu(kind, None, Some(hints::Stop::List), window, cx);
        self.highlight_first();
    }

    fn aim_detail(&mut self) {
        if self.ui.compact {
            return;
        }
        let placement = self.detail_placement();
        self.motion.detail.aim(
            self.ui
                .task
                .filter(|_| !self.ai.open || placement != DetailPane::Hidden),
            placement == DetailPane::Docked,
            self.width == Width::Narrow || (self.ai.open && !self.ai_shares_detail()),
            self.now(),
            self.pace(),
        );
    }

    fn settle_detail(&mut self) {
        if self.ui.compact {
            return;
        }
        let placement = self.detail_placement();
        self.motion.detail = DetailMotion::rest(
            self.shell
                .detail
                .as_ref()
                .map(|d| d.task)
                .filter(|_| !self.ai.open || placement != DetailPane::Hidden),
            placement == DetailPane::Docked,
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
        self.ai.set_colors(&self.palette, cx);
        let colors = quick_add_colors(&self.palette);
        for input in [
            &self.quick_add,
            &self.filter,
            &self.edit.step,
            &self.edit.link,
        ] {
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

    fn start_room_switch(&mut self, next: &Shell, body: Option<OutgoingBody>) {
        if let Some(body) = body {
            self.begin_navigation(next, body);
        }
        self.rows_bar
            .reset_to_top(&self.rows.0.borrow().base_handle);
        self.rows.scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.handover_bar.reset_to_top(&self.handover_scroll);
        self.trash_selected = None;
    }

    fn begin_navigation(&mut self, next: &Shell, body: OutgoingBody) {
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
        let body = old.filter(|_| keep.list).map_or(body, |old| old.body);
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

    /// Where a screenshot fixture puts the keyboard: an open menu, an editor it opened, a field it typed
    /// into, the quick-add when it has typed text, else the list, as in the running app.
    pub fn fixture_focus(&self, cx: &App) -> FocusHandle {
        let quick_add = self.quick_add.read(cx);
        if self.menu.is_some() {
            self.menu_focus.clone()
        } else if let Some(inline) = &self.inline {
            inline.input.read(cx).focus_handle.clone()
        } else if let Some(focus) = &self.preview_focus {
            focus.clone()
        } else if quick_add.content.is_empty() {
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
        self.my_day_progress.counts()
    }

    fn submit(&mut self, cmds: Vec<todo_core::Command>) -> bool {
        for cmd in cmds {
            let edit = undo::EditKind::of(&cmd).filter(|(entity, kind)| {
                *kind != undo::EditKind::Reopen || matches!(entity, todo_core::ids::EntityId::Task(id) if self.task_stage(*id).is_some())
            });
            let result = if let Some((entity, kind)) = edit {
                let state = self.store.view();
                let label = undo::toast_label(&state, &cmd, entity, kind);
                let room = self.shell.room.key;
                self.store.submit_undoable(cmd).map(|target| {
                    // Deleting the list (or collection) the room shows takes the window to My Day: the
                    // undo goes along.
                    let room = if kind.leaves_room() {
                        self.ui.room.resolved(&self.store.view())
                    } else {
                        room
                    };
                    self.remember_undo(
                        undo::Action {
                            entity,
                            kind,
                            targets: vec![target],
                            room,
                        },
                        label,
                    );
                })
            } else {
                let stale = self
                    .last_undo
                    .as_ref()
                    .is_some_and(|u| undo::touches(&cmd, u.entity, &self.store.view()));
                self.store.submit(cmd).map(|()| {
                    if stale {
                        self.clear_undo();
                    }
                })
            };
            if let Err(e) = result {
                tracing::warn!("a change was refused: {e}");
                self.notice = Some(Notice::error(format!("Couldn't save that: {e}")));
                return false;
            }
        }
        self.notice = None;
        true
    }

    /// Submits one undoable command only if `guard` holds on the state at the moment it is queued
    /// (`Store::submit_undoable_if`), with its undo toast. `Ok(false)` means the guard refused;
    /// `Err(())` means the command failed, with its error already in the status bar.
    fn submit_if(
        &mut self,
        cmd: todo_core::Command,
        guard: impl FnOnce(&todo_core::State) -> bool,
    ) -> Result<bool, ()> {
        let Some((entity, kind)) = undo::EditKind::of(&cmd) else {
            return Err(());
        };
        self.submit_if_kind(cmd, entity, kind, guard)
    }

    fn submit_if_kind(
        &mut self,
        cmd: todo_core::Command,
        entity: todo_core::ids::EntityId,
        kind: undo::EditKind,
        guard: impl FnOnce(&todo_core::State) -> bool,
    ) -> Result<bool, ()> {
        let label = undo::toast_label(&self.store.view(), &cmd, entity, kind);
        let room = self.shell.room.key;
        match self.store.submit_undoable_if(cmd, guard) {
            Ok(Some(target)) => {
                let action = undo::Action {
                    entity,
                    kind,
                    targets: vec![target],
                    room,
                };
                self.remember_undo(action, label);
                self.notice = None;
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => {
                tracing::warn!("a change was refused: {e}");
                self.notice = Some(Notice::error(format!("Couldn't save that: {e}")));
                Err(())
            }
        }
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
        self.add_draft(
            &quick_add::Draft {
                text: text.into(),
                ..Default::default()
            },
            room,
            cx,
        )
    }

    fn add_draft(
        &mut self,
        draft: &quick_add::Draft,
        room: RoomKey,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = self.store.now();
        let store = self.store.clone();
        let (id, cmds) =
            commands::submit_workflow_draft(&store.view(), room, draft, &now.eval(), || {
                store.new_uuid()
            });
        let ok = id.is_some() && self.submit(cmds);
        self.refresh(cx);
        ok
    }

    pub fn select_room(&mut self, room: RoomKey, cx: &mut Context<Self>) {
        self.cancel_task_drag(cx);
        if !self.commit_edits(cx) && self.ui.room != room {
            self.keep_unsaved(cx);
            return;
        }
        self.multi.clear();
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
        self.multi.clear();
        self.ui.task = id;
        if self.ai_docked() {
            self.ai.detail_over_room = id.is_some();
        }
        self.refresh(cx);
        if self.on_board() {
            if let Some(id) = id {
                self.aim_board_cursor(id);
            }
        } else if self.on_timeline() {
            self.timeline.cursor = id;
        } else if let Some(i) = self.shell.selected_task {
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
        let was_done = task.is_done();
        if was_done
            && self.motion.completing(id)
            && self
                .last_undo
                .as_ref()
                .is_some_and(|u| u.entity == id.into() && u.kind == undo::EditKind::Complete)
        {
            self.request_undo(cx);
            return;
        }
        let Some(cmd) = commands::toggle_complete(&state, id) else {
            return;
        };
        if self.submit(vec![cmd]) {
            self.play_completion(task);
        }
        self.refresh(cx);
    }

    fn play_completion(&mut self, task: &todo_core::model::Task) {
        if !self.animate {
            return;
        }
        let (now, pace) = (self.now(), self.pace());
        let keep = if task.is_done() {
            self.motion.reopen(task.id, now, pace)
        } else {
            self.motion.complete(task.id, now, pace)
        };
        if keep == Keep::Holding {
            self.ui.held_updates.insert(task.id, task.updated.clone());
        } else if keep == Keep::Reopening
            && let Some(at) = task.completed_at
        {
            self.ui.reopening.insert(task.id, at);
        }
    }

    fn toggle_task_glyph(&mut self, id: TaskId, icon: Icon, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(task) = state.tasks.get(&id) else {
            return;
        };
        let today = self.store.now().today();
        let (on, command) = if icon == Icon::Sun {
            (
                task.my_day != Some(today),
                commands::toggle_my_day(&state, id, today),
            )
        } else {
            (
                task.priority != todo_core::model::Priority::High,
                commands::toggle_important(&state, id),
            )
        };
        if let Some(command) = command {
            use todo_design::motion as m;
            let start = self.now();
            let reduced = self.pace().reduced;
            let from = self
                .task_toggles
                .get(&(id, icon))
                .map_or(f32::from(!on), |a| a.at(start).0);
            let ms = if reduced {
                m::reduced(m::CHECK_FILL, true)
            } else {
                m::CHECK_FILL
            };
            self.task_toggles.insert(
                (id, icon),
                ToggleMotion {
                    start,
                    span: Duration::from_millis(u64::from(ms)),
                    from,
                    on,
                    reduced,
                },
            );
            self.submit(vec![command]);
            self.refresh(cx);
        }
    }

    fn task_glyph(&self, p: &Palette, task: TaskId, icon: Icon, on: bool, size: f32) -> Div {
        let (fill, scale) = self
            .task_toggles
            .get(&(task, icon))
            .filter(|a| a.on == on)
            .map_or((f32::from(on), 1.), |a| a.at(self.frame));
        kit::task_toggle(p, icon, size, fill, scale)
    }

    fn cycle_priority(&mut self, id: TaskId, cx: &mut Context<Self>) {
        if let Some(cmd) = commands::cycle_priority(&self.store.view(), id) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// Deletes a task, without asking: the toast's Undo (or Ctrl+Z) brings it back. Its row leaves with
    /// the completion's exit, and the selection goes to the row that takes its place.
    fn delete_task(&mut self, id: TaskId, cx: &mut Context<Self>) {
        let cmd = commands::delete_task(&self.store.view(), id);
        self.change_task(id, cmd, cx);
    }

    /// Moves a task to another list, first in its manual order. A row that leaves the room (another
    /// list's room) leaves with the completion's exit; in a smart list it stays where it is.
    fn move_task_to(&mut self, id: TaskId, from: ListId, list: ListId, cx: &mut Context<Self>) {
        let cmd = commands::organize::move_task(&self.store.view(), id, from, list);
        self.change_task(id, cmd, cx);
    }

    fn merge_task(
        &mut self,
        source: TaskId,
        target: TaskId,
        question: &str,
        cx: &mut Context<Self>,
    ) {
        use todo_client::intent::merge;
        self.change_task_with(
            source,
            |this| {
                let queued = merge::command(&this.store.view(), source, target, question).map_or(
                    Ok(false),
                    |cmd| {
                        this.submit_if(cmd, |state| {
                            merge::command(state, source, target, question).is_some()
                        })
                    },
                );
                if queued == Ok(false) {
                    this.notice = Some(Notice::error(merge::CHANGED));
                }
                queued == Ok(true)
            },
            cx,
        );
    }

    fn change_task(&mut self, id: TaskId, cmd: Option<todo_core::Command>, cx: &mut Context<Self>) {
        let Some(cmd) = cmd else {
            return self.refresh(cx);
        };
        self.change_task_with(id, |this| this.submit(vec![cmd]), cx);
    }

    /// If an edit takes `id` out of the room, keep its row in place while it leaves and hand
    /// selection to its neighbour.
    fn change_task_with(
        &mut self,
        id: TaskId,
        submit: impl FnOnce(&mut Self) -> bool,
        cx: &mut Context<Self>,
    ) {
        // A draft of this task's title or notes is saved first, so Undo brings it back as it was typed.
        if !self.commit_edits(cx) {
            return self.keep_unsaved(cx);
        }
        let state = self.store.view();
        // Its row or its line in the finished day's hand-over, as drawn now.
        let shown = self.shell.shown_row(id).map(|r| r.done);
        let snapshot = state.tasks.get(&id).cloned().map(|mut task| {
            // As the list places it now: still open while its completion holds, still done while
            // its reopening plays.
            if self.motion.completing(id) {
                task.completed_at = None;
            }
            if let Some(at) = self.ui.reopening.get(&id) {
                task.completed_at = Some(*at);
            }
            task
        });
        let neighbour = self.neighbour(id);
        if !submit(self) {
            return self.refresh(cx);
        }
        let after = model::derive(
            &self.store.view(),
            &self.ui,
            &self.store.sync(),
            &self.store.now(),
        );
        let gone = after.shown_row(id).is_none();
        if gone
            && self.animate
            && let (Some(done), Some(task)) = (shown, snapshot)
        {
            let (now, pace) = (self.now(), self.pace());
            self.motion.leave(id, done, now, pace);
            self.ui.leaving.insert(id, task);
        }
        if gone && self.ui.task == Some(id) {
            self.ui.task = neighbour;
        }
        self.refresh(cx);
        if let Some(i) = self.shell.selected_task.filter(|_| gone) {
            self.rows.scroll_to_item(i, gpui::ScrollStrategy::Center);
        } else if let (true, Some(next), Some(id)) =
            (gone, self.shell.room.handover(), self.ui.task)
            && let Some(child) = next.child(id)
        {
            self.handover_scroll.scroll_to_item(child);
        }
    }

    /// The task J or K reaches from `id` (the next one, else the one before): where the selection goes
    /// when `id` leaves.
    fn neighbour(&self, id: TaskId) -> Option<TaskId> {
        let tasks = self.nav_tasks();
        let at = tasks.iter().position(|&t| t == id)?;
        tasks
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|i| tasks.get(i)))
            .copied()
    }

    /// Where a task's menu opened from the keyboard hangs: under its row's title, where the row was last
    /// painted. A task without a row (open in the detail pane only) gets it at the list's top.
    fn row_anchor(&self, id: TaskId, window: &Window) -> Point<Pixels> {
        let indent = desktop::LIST_PAD_X + desktop::COL_CHECK + desktop::ROW_GAP;
        match self.row_bounds.borrow().get(&id) {
            Some(b) => gpui::point(b.origin.x + px(indent), b.origin.y + b.size.height),
            None => {
                let left = self.columns(window).width(Pane::Sidebar) + indent;
                gpui::point(px(left), px(desktop::TITLEBAR + desktop::ROW * 3.))
            }
        }
    }

    /// M or Shift+F10: the selected task's list picker or context menu, from the keyboard, under its
    /// row. The keyboard comes back to the list once something is chosen.
    fn open_task_menu(&mut self, page: TaskPage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.ui.task.filter(|&id| self.task_is_visible(id)) else {
            return;
        };
        let at = self.row_anchor(id, window);
        let kind = MenuKind::Task { id, page };
        self.open_menu(kind, Some(at), Some(hints::Stop::List), window, cx);
        self.highlight_first();
    }

    /// J and K: through the rows, or through the finished day's hand-over to tomorrow.
    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let tasks = self.nav_tasks();
        let n = tasks.len();
        if n == 0 {
            return;
        }
        let at = self
            .multi
            .cursor()
            .or(self.ui.task)
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
        self.activity.motion.settle();
    }

    fn titlebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let filtering = !self.ui.filter.is_empty();
        let focused = self.filter.read(cx).focus_handle.is_focused(window);
        let filter_width = 440.;
        let filter_height = desktop::TITLEBAR - space::S2 - space::S1 / 2.;
        div()
            .relative()
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
                            .style(ty::NAV_COLLECTION)
                            .text_color(p.ink)
                            .child("To-do"),
                    ),
            )
            .when(self.shell.room.key != RoomKey::Trash, |bar| {
                bar.child(
                    div()
                        .absolute()
                        .left((window.viewport_size().width - px(filter_width)) / 2.)
                        .top(px((desktop::TITLEBAR - filter_height) / 2.))
                        .w(px(filter_width))
                        .h(px(filter_height))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .pl(px(space::S2))
                        .pr(px(space::S1))
                        .rounded(px(radius::MD))
                        .border(px(elevation::HAIRLINE))
                        .border_color(if focused { p.accent_strong } else { p.control })
                        .bg(if filtering { p.wash } else { p.pane })
                        .when(focused, |d| {
                            d.shadow(vec![gpui::BoxShadow {
                                color: p.tint,
                                offset: gpui::point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(elevation::FOCUS_RING),
                                inset: false,
                            }])
                        })
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
            })
            .child(self.ai_trigger(cx))
            .child(kit::window_controls(p, window, self.caption_preview).flex_none())
    }

    fn eyebrow(&self, eyebrow: &Eyebrow) -> Div {
        let (_, ink_3, line) = self.header_inks();
        let accent = if self.motion.dusk(self.frame).night_ink() {
            self.dusk_accent()
        } else {
            self.palette.accent_strong
        };
        let bar = || {
            div()
                .w(px(elevation::HAIRLINE))
                .h(px(ty::CAPS_LABEL.size - 0.5))
                .flex_none()
                .bg(line)
        };
        let mono = |s: &SharedString| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .style(ty::EYEBROW_META)
                .text_color(ink_3)
                .child(s.to_uppercase())
        };
        let lead = |s: SharedString| {
            div()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .style(ty::EYEBROW_DAY)
                .text_color(accent)
                .child(s)
        };
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
                .child(lead(weekday.clone()))
                .child(bar())
                .child(mono(date))
                .child(bar())
                .child(mono(week)),
            Eyebrow::Kind { kind, detail } => row
                .child(lead(kind.clone()))
                .child(bar())
                .child(mono(detail))
                .when(self.on_timeline(), |d| {
                    d.child(bar()).child(mono(
                        &format!("{} scheduled", self.timeline_counts().0).into(),
                    ))
                }),
        }
    }

    fn summary(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
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
        let figure = |n: usize| {
            div()
                .style(ty::SUMMARY_FIGURES.with_weight(600))
                .text_color(ink)
                .child(n.to_string())
        };
        let done_of = |done: usize, total: usize| {
            div()
                .flex()
                .items_baseline()
                .gap(px(space::S1))
                .child(figure(done))
                .child(format!("of {total} done"))
        };
        // The shared summary (`todo_client::room_header`) says what to show; this draws it.
        row = match room.score {
            // Changes aren't scored: the feed itself says what happened.
            Summary::None => return row,
            Summary::Empty { .. } => {
                return row.child(div().style(ty::SUMMARY_FIGURES).child(room.score.text()));
            }
            Summary::Done { done } => row.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::S1))
                    .child(figure(done))
                    .child("done"),
            ),
            Summary::AllClear { done, total } => {
                row.child(done_of(done, total)).child(sep()).child(
                    // The day is done: the count says so in the accent, without congratulations.
                    div()
                        .style(ty::SUMMARY_FIGURES.with_weight(600))
                        .text_color(self.dusk_accent())
                        .child("all clear"),
                )
            }
            Summary::Progress { done, total, open } => {
                row.child(done_of(done, total)).child(sep()).child(
                    div()
                        .style(ty::SUMMARY_FIGURES)
                        .child(format!("{open} open")),
                )
            }
        };
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

    /// The room header's "more" button, when the room's menu offers something (`model::menus`). There
    /// are no view options yet, so there is no sliders button.
    fn room_menu_button(&self, ink: Hsla, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        if !self.has_room_menu() {
            return None;
        }
        let p = &self.palette;
        let open = self.menu.as_ref().is_some_and(|m| {
            matches!(m.kind, MenuKind::Room { .. }) && m.overlay.look(self.frame).open
        });
        let hover = p.hover;
        let button = div()
            .id("room-menu")
            .relative()
            .size(px(component::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.room_menu_focus)
            .when(open, |d| d.bg(hover))
            .hover(move |s| s.bg(hover))
            .child(kit::icon(Icon::Ellipsis, component::ICON, ink))
            .on_click(cx.listener(|this, e: &gpui::ClickEvent, window, cx| {
                this.toggle_menu(
                    MenuKind::Room { page: Page::Main },
                    hints::Stop::RoomMenu,
                    e,
                    window,
                    cx,
                );
            }))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                this.control_key(e, window, cx);
            }))
            .children(self.menu_anchor(|k| matches!(k, MenuKind::Room { .. }), cx));
        Some(menu::ring(
            p,
            self.room_menu_focus.is_focused(window),
            radius::MD,
            button,
        ))
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
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
        // Ellipsize in text layout rather than clipping both axes with `overflow_x_hidden`.
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
            .flex_1()
            .min_w_0()
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
                    .right_0()
                    .top(px(swap.outgoing_y))
                    .opacity(swap.outgoing)
            }));
        let more = self.room_menu_button(ink_3, window, cx);
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
                            .gap(px(space::S2))
                            .child(eyebrow)
                            .children(self.view_switcher(window, cx))
                            .children(more.map(|d| d.flex_none().mr(px(-space::S2)))),
                    )
                    .child(if self.columns(window).counts_under_title() {
                        // A narrow room can keep the whole line for the title, with counts below it.
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

    fn quick_add_field(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let focused = self.quick_focus.input.is_focused(window);
        let typed = !self.quick_draft.text.is_empty();
        let parsed = self.quick_preview();
        let date = self.quick_date_label(&parsed);
        let has_date = date.is_some();
        let priority = parsed.priority.unwrap_or_default();
        let workflow = self.shell.room.features.workflow;
        let stage = self.quick_stage();
        let status_chip = workflow && stage != todo_core::query::QueryStatus::Todo;
        let has_chips = has_date || priority != Priority::None || status_chip;
        // On the finished day the field stays, but quietly: on the pane with a hairline, so the hand-over
        // to tomorrow leads. Ctrl+N or a click wakes it up.
        let quiet = self.shell.room.finished_day() && !focused && !typed;
        let field = div()
            .h(px(desktop::QUICK_ADD))
            .flex()
            .items_center()
            .gap(px(space::S2))
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
            .child(div().flex_1().min_w_0().child(self.quick_add.clone()))
            .when(status_chip, |field| {
                field.child(self.quick_status_chip(stage, window, cx))
            })
            .children(date.map(|label| self.quick_date_chip(label, window, cx)))
            .when(priority != Priority::None, |field| {
                field.child(self.quick_priority_chip(priority, window, cx))
            })
            .when(workflow, |field| {
                field.child(self.quick_status_button(window, cx))
            })
            .child(self.quick_picker_button(true, has_date, window, cx))
            .child(self.quick_picker_button(false, priority != Priority::None, window, cx))
            .when(!has_chips, |field| {
                field.child(kit::kbd(p, if typed { "Enter" } else { "Ctrl N" }))
            });
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
        Self::row_slot(self.row(task, style, window, cx), look, lift)
    }

    fn row_slot(row: Stateful<Div>, look: Option<RowLook>, lift: f32) -> AnyElement {
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
        if self.ai.open && (self.width == Width::Narrow || !self.ai_shares_detail()) {
            return if self.ai_docked() && self.ai.detail_over_room && self.shell.detail.is_some() {
                DetailPane::Overlay
            } else {
                DetailPane::Hidden
            };
        }
        DetailPane::at(
            self.width,
            self.shell.detail.is_some(),
            self.shell.room.leaves_detail_out()
                || self.shell.room.features.workflow
                || self.shell.room.features.timeline,
        )
    }

    fn docked(&self) -> bool {
        self.width == Width::Wide
            && (!self.ai.open || self.ai_shares_detail())
            && self.motion.detail.look(self.frame).docked
    }

    /// The room pane's width (and so a row's): the window less the sidebar and a docked detail pane.
    fn room_width(&self, window: &Window) -> f32 {
        if self.ui.compact {
            window.viewport_size().width.into()
        } else {
            self.columns(window).room_width()
        }
    }

    /// The narrow window's detail: over the right part of the room, with a scrim over the rest of it.
    /// It slides in from the right and fades in (`OVERLAY_IN`), unless the window narrowed with the task
    /// already open, and fades out when closed (`OVERLAY_OUT`); with reduced motion it only fades,
    /// briefly. Clicking the scrim closes it and leaves the keyboard on the list. A pane fading out is
    /// inert: a shield over it swallows every click and hover, so nothing in it acts on the dismissed
    /// task, and the scrim still keeps clicks off the rows underneath.
    fn detail_overlay(&self, window: &Window, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let look = self.motion.detail.look(self.frame);
        let beside_ai = self.ai_docked()
            && !self.ai_shares_detail()
            && (self.ai.detail_over_room || self.closing.is_some());
        if (self.width != Width::Narrow && !beside_ai)
            || (self.width == Width::Narrow && self.ai.open)
            || look.task == 0.
        {
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
    fn meta(
        p: &Palette,
        task: &TaskRow,
        chips: Chips,
        room: f32,
        mark: Option<(f32, AnyElement)>,
        window: &Window,
    ) -> Div {
        let t = p.theme;
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
        let fit = fit::fit_meta(
            room - mark
                .as_ref()
                .map_or(0., |(width, _)| width.ceil() + items.gap),
            &items,
            more_width,
        );
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
            .children(mark.map(|(_, mark)| mark))
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
        let RowStyle {
            selected,
            room,
            look,
            chips,
        } = style;
        let id = task.id;
        let bulk_selected = room.is_some() && self.multi.contains(id);
        let selected = selected || bulk_selected;
        let p = &self.palette.for_task_row(bulk_selected);
        let t = self.theme;
        let overdue = matches!(task.due, Due::Overdue(_));
        // The trailing columns keep their places from the right edge, but empty ones to the left of every
        // filled one are left out, so the title runs on into their room instead of stopping short.
        let show_agent = task.agent.is_some();
        let show_priority = show_agent || task.priority > 0;
        let full_due = show_priority || !matches!(task.due, Due::None);
        // The title column's width: the row less its padding, the checkbox and the trailing columns.
        let room_width = self.room_width(window);
        let mut title_width = room_width - 2. * desktop::LIST_PAD_X - desktop::COL_CHECK;
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
        title_width -= 2. * desktop::ROW_GAP + component::ICON_BUTTON;
        let meta_width = fit::row_meta_width(room_width, title_width);
        let status = task.status.filter(|stage| {
            chips == Chips::None
                || matches!(
                    stage,
                    todo_core::query::QueryStatus::Backlog
                        | todo_core::query::QueryStatus::InProgress
                )
        });
        let mark = status.map(|stage| {
            (
                workflow::mark_width(window, stage),
                self.row_status_mark(stage, selected, room.map(|r| (r, id)), p, cx),
            )
        });
        let meta = Self::meta(p, task, chips, meta_width, mark, window);
        let (meta, compact_meta) = if meta_width > title_width {
            // The trailing columns occupy only the title line. Fit metadata under them when
            // docked detail leaves the room too narrow for the regular layout.
            (
                None,
                Some(
                    meta.absolute()
                        .left(px(desktop::LIST_PAD_X
                            + desktop::COL_CHECK
                            + desktop::ROW_GAP))
                        .top(px(desktop::ROW_PAD_TOP + ty::TASK_TITLE.line_height))
                        .w(px(meta_width)),
                ),
            )
        } else {
            (Some(meta), None)
        };
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
                el.on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if this.row_can_act(room, id)
                        && !this.modified_task_click(id, click.modifiers(), cx)
                    {
                        this.toggle_complete(id, cx);
                    }
                }))
            });
        let title = Self::struck_title(
            p,
            ty::TASK_TITLE,
            &task.title,
            look.unwrap_or(RowLook::rest(task.done)),
        );
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
                    .children(meta),
            )
            .children(compact_meta)
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
                            el.on_click(cx.listener(
                                move |this, click: &gpui::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    if this.row_can_act(room, id)
                                        && !this.modified_task_click(id, click.modifiers(), cx)
                                    {
                                        this.cycle_priority(id, cx);
                                    }
                                },
                            ))
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
            .child({
                let on = task.priority == 3;
                div()
                    .id(SharedString::from(format!("important-{id}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(component::ICON_BUTTON))
                    .mt(px((component::CHIP - component::ICON_BUTTON) / 2.))
                    .rounded(px(radius::BUTTON))
                    .child(self.task_glyph(p, id, Icon::Star, on, component::ICON))
                    .when_some(room, |el, room| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(p.hover))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Important: {}", task.title))
                            .aria_toggled(if on {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .tooltip(kit::tooltip(
                                *p,
                                if on {
                                    "Remove importance"
                                } else {
                                    "Mark as important"
                                },
                            ))
                            .on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                if this.row_can_act(room, id)
                                    && !this.modified_task_click(id, click.modifiers(), cx)
                                {
                                    this.toggle_task_glyph(id, Icon::Star, cx);
                                }
                            }))
                    })
            })
            .when_some(room, |el, room| {
                el.on_click(
                    cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        if this.row_can_act(room, id) {
                            window.focus(&this.list_focus, cx);
                            if !this.modified_task_click(id, event.modifiers(), cx) {
                                this.select_task(Some(id), cx);
                                if this.ui.compact && event.click_count() >= 2 {
                                    this.show_task_in_main(id, cx);
                                }
                            }
                        }
                    }),
                )
                // Right-click: the task's context menu, at the pointer. The row isn't selected.
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        if this.row_can_act(room, id) && this.multi.cursor().is_none() {
                            cx.stop_propagation();
                            let kind = MenuKind::Task {
                                id,
                                page: TaskPage::Main,
                            };
                            this.open_menu(kind, Some(e.position), None, window, cx);
                        }
                    }),
                )
                .when(!self.ui.compact || matches!(room, RoomKey::List(_)), |el| {
                    el.child(self.drag_handle(id, room, task.title.clone(), cx))
                })
                // Where it is, so a menu opened from the keyboard can hang under it.
                .child(self.row_bounds_probe(id))
            });
        // The row a context menu is open on keeps the hover look while the menu is up.
        let menu_target = self
            .menu
            .as_ref()
            .filter(|m| m.overlay.look(self.frame).open)
            .and_then(|m| m.kind.task())
            == Some(id);
        let cursor =
            room.is_some() && self.multi.cursor() == Some(id) && self.list_focus.is_focused(window);
        let row = row.when(bulk_selected || cursor, |row| {
            row.role(gpui::Role::Button)
                .aria_label(task.title.clone())
                .aria_toggled(if bulk_selected {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
        });
        let row = self.task_selection(row, selected, bulk_selected, cursor);
        let hover = p.hover;
        row.when(!selected && room.is_some(), |row| {
            row.hover(move |s| s.bg(hover))
                .when(menu_target, |row| row.bg(hover))
        })
        .when_some(
            room.and_then(|_| self.task_settle_alpha(id).or_else(|| self.ai_flash(id))),
            |row, a| {
                let base = if bulk_selected {
                    p.selection_wash
                } else if selected {
                    p.wash
                } else {
                    p.pane
                };
                row.bg(theme::mix(base, p.wash, a))
            },
        )
        .when(room.is_some() && self.row_lifted(id), |row| {
            row.opacity(1. - elevation::DROP_WASH_ALPHA)
        })
        .when_some(room.and_then(|_| self.drag_slot(id)), |row, after| {
            row.child(
                div()
                    .absolute()
                    .left(px(space::S2))
                    .right(px(space::S2))
                    .when(after, |line| line.bottom_0())
                    .when(!after, |line| line.top_0())
                    .h(px(elevation::SELECTION_BAR))
                    .rounded(px(radius::BUTTON))
                    .bg(p.accent_strong),
            )
        })
    }

    /// Records where task `id`'s row is painted (`row_bounds`), without drawing anything.
    fn row_bounds_probe(&self, id: TaskId) -> impl IntoElement {
        let bounds = self.row_bounds.clone();
        gpui::canvas(
            move |b, _, _| {
                bounds.borrow_mut().insert(id, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Records where a control was painted, for `scroll_into_view`.
    fn probe(&self, key: &'static str) -> impl IntoElement {
        let probes = self.probes.clone();
        gpui::canvas(
            move |b, _, _| {
                probes.borrow_mut().insert(key, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Scrolls the pane of `handle` so the control `key` (a `probe`) is in view, a small margin clear of
    /// its edges (`layout::reveal_offset`). Nothing when it's in view, or hasn't been painted yet.
    fn scroll_into_view(&self, handle: &ScrollHandle, key: &'static str) -> bool {
        self.probes
            .borrow()
            .get(key)
            .is_some_and(|&target| self.reveal_bounds(handle, target))
    }

    fn reveal_bounds(&self, handle: &ScrollHandle, target: Bounds<Pixels>) -> bool {
        let view = handle.bounds();
        let offset = handle.offset();
        let next = model::layout::reveal_offset(
            (f32::from(view.top()), f32::from(view.bottom())),
            (f32::from(target.top()), f32::from(target.bottom())),
            f32::from(offset.y),
            f32::from(handle.max_offset().y),
            space::S2,
        );
        if let Some(y) = next {
            handle.set_offset(gpui::point(offset.x, px(y)));
        }
        next.is_some()
    }

    /// A task's title in any of its layouts (a list row, a line of the hand-over, the hand-over's first
    /// task), as its look `l` paints it (`RowLook::rest` when nothing plays): the ink dims to `ink-3`
    /// and the strike-through draws from the left in step with the tick. At rest, and once drawn, it's
    /// GPUI's own strike-through; part way, the same line again, invisible but for its strike-through,
    /// cut off at `strike` of the line's own width (never the column's), so it lands where the rest
    /// state draws it.
    fn struck_title(p: &Palette, style: ty::TextStyle, title: &SharedString, l: RowLook) -> Div {
        let ink = theme::mix(p.ink, p.ink_3, l.dim);
        let line = || {
            div()
                .min_w_0()
                .truncate()
                .style(style)
                .text_color(ink)
                .child(title.clone())
        };
        if l.strike >= 1. {
            return line().line_through();
        }
        if l.strike <= 0. {
            return line();
        }
        let mut strike = div()
            .whitespace_nowrap()
            .style(style)
            .text_color(gpui::transparent_black())
            .child(title.clone());
        strike.text_style().strikethrough = Some(StrikethroughStyle {
            thickness: px(elevation::HAIRLINE),
            color: Some(ink),
        });
        // In a row of its own, the line takes its own width (up to the column's), not the column's.
        div().min_w_0().flex().child(
            div().relative().min_w_0().child(line()).child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(gpui::relative(l.strike))
                    .overflow_hidden()
                    .child(strike),
            ),
        )
    }

    fn capture_body(&self, min_height: f32) -> OutgoingBody {
        let room = &self.shell.room;
        if room.key == RoomKey::Trash && !room.trash.is_empty() {
            let state = self.rows.0.borrow();
            let viewport = f32::from(state.base_handle.bounds().size.height).max(min_height);
            let scroll = (-f32::from(state.base_handle.offset().y)).max(0.);
            let first = (scroll / desktop::ROW).floor() as usize;
            let cut = scroll - first as f32 * desktop::ROW;
            let visible = ((viewport + cut) / desktop::ROW).ceil() as usize;
            OutgoingBody::Trash {
                rows: room
                    .trash
                    .iter()
                    .skip(first)
                    .take(visible)
                    .cloned()
                    .collect(),
                cut,
            }
        } else if room.key.is_activity() && !self.activity.feed.is_empty_feed() {
            OutgoingBody::Activity {
                days: self.activity_days(),
                top: self.activity_scroll.offset().y,
            }
        } else if let Some(next) = room.handover() {
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
            let end = first + visible;
            let rows = (first..end)
                .chain(rows_rising_into(&self.row_lifts, end))
                .map_while(|i| {
                    let task = self.shell.row(i)?;
                    Some(OutgoingRow {
                        task: task.clone(),
                        selected: self.shell.selected_task == Some(i),
                        look: self.motion.row(task.id, self.frame),
                        lift: self.row_lifts.get(i).copied().unwrap_or(0.),
                    })
                })
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
                    .text_color(self.palette.ink_2)
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
                .children(rows.iter().map(|row| {
                    let content = self.row(
                        &row.task,
                        RowStyle {
                            room: None,
                            selected: row.selected,
                            chips: *chips,
                            look: row.look,
                        },
                        window,
                        cx,
                    );
                    Self::row_slot(content, row.look, row.lift)
                }))
                .into_any_element(),
            OutgoingBody::Empty { line, hint } => self.empty_list(line, hint).into_any_element(),
            OutgoingBody::Handover { tomorrow, top } => self
                .tomorrow(tomorrow, Some(*top), window, cx)
                .into_any_element(),
            OutgoingBody::Plan(weekday) => self
                .plan_only(weekday.clone(), false, cx)
                .into_any_element(),
            OutgoingBody::Trash { rows, cut } => div()
                .relative()
                .top(px(-cut))
                .children(rows.iter().map(|row| self.trash_row(row, None)))
                .into_any_element(),
            OutgoingBody::Activity { days, top } => {
                self.activity_snapshot(days, *top, cx).into_any_element()
            }
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
            && (self.shell.row_count() > 0 || !room.trash.is_empty());
        // The finished day hands over to tomorrow, unless the Completed section is open: then its rows show.
        let body: AnyElement = if room.key.is_activity() {
            self.activity_feed(window, cx)
        } else if room.key == RoomKey::Trash {
            self.trash_feed(cx)
        } else if let Some(next) = room.handover() {
            self.tomorrow(next, None, window, cx).into_any_element()
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
                    .on_action(cx.listener(|this, _: &NextTask, window, cx| {
                        if this.in_activity() {
                            this.feed_input(FeedInput::Next, false, window, cx);
                        } else if this.shell.room.key == RoomKey::Trash {
                            this.trash_step(1, cx);
                        } else {
                            this.move_selection(1, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PreviousTask, window, cx| {
                        if this.in_activity() {
                            this.feed_input(FeedInput::Previous, false, window, cx);
                        } else if this.shell.room.key == RoomKey::Trash {
                            this.trash_step(-1, cx);
                        } else {
                            this.move_selection(-1, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &SelectAllTasks, _, cx| {
                        this.change_selection(todo_client::selection::Selection::all, cx);
                    }))
                    .on_action(cx.listener(|this, _: &ExtendTasksUp, _, cx| {
                        this.change_selection(|s, rows, previous| s.extend(rows, -1, previous), cx);
                    }))
                    .on_action(cx.listener(|this, _: &ExtendTasksDown, _, cx| {
                        this.change_selection(|s, rows, previous| s.extend(rows, 1, previous), cx);
                    }))
                    // Space completes or reopens the selected task, and Delete deletes it (`list_key`):
                    // a held key's repeats are consumed without acting.
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                        // In Activity, Enter (or U) undoes the selected change.
                        if this.in_activity() {
                            let key = FeedInput::Key {
                                key: e.keystroke.key.as_str(),
                                modified: e.keystroke.modifiers.modified(),
                            };
                            if this.feed_input(key, e.is_held, window, cx) {
                                cx.stop_propagation();
                            }
                            return;
                        }
                        if this.shell.room.key == RoomKey::Trash {
                            this.trash_key(e, window, cx);
                            return;
                        }
                        if this.workflow_key(e, window, cx) {
                            cx.stop_propagation();
                            return;
                        }
                        let Some((key, act)) = list_key(e) else {
                            return;
                        };
                        cx.stop_propagation();
                        let Some(id) = this.ui.task.filter(|&id| act && this.task_is_visible(id))
                        else {
                            return;
                        };
                        match key {
                            ListKey::Toggle => this.toggle_complete(id, cx),
                            ListKey::Delete => this.delete_task(id, cx),
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Escape, window, cx| {
                        if this.cancel_task_drag(cx) {
                            return;
                        }
                        if this.multi.cursor().is_some() {
                            this.select_task(None, cx);
                        } else if this.ui.compact {
                            this.close_popout(window);
                        } else if this.in_activity() {
                            this.feed_input(FeedInput::Escape, false, window, cx);
                        } else if this.shell.room.key == RoomKey::Trash {
                            this.trash_selected = None;
                            cx.notify();
                        } else if this.ai.open {
                            this.escape_ai(window, cx);
                        } else {
                            this.select_task(None, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &EditTask, window, cx| {
                        if this.ui.compact {
                            if let Some(task) = this.ui.task {
                                this.show_task_in_main(task, cx);
                            }
                        } else if this.ui.task.is_some() {
                            let handle = this.edit.title.read(cx).focus_handle.clone();
                            this.edit.title.update(cx, |a, cx| a.select_everything(cx));
                            window.focus(&handle, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &EditDue, window, cx| {
                        if !this.ui.compact {
                            this.start_due(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &CyclePriority, _, cx| {
                        if let Some(id) = this.ui.task {
                            this.cycle_priority(id, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PickPriority, window, cx| {
                        this.pick_priority(window, cx);
                    }))
                    .on_action(
                        cx.listener(|this, _: &MoveTaskUp, _, cx| this.nudge_task_drag(false, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &MoveTaskDown, _, cx| this.nudge_task_drag(true, cx)),
                    )
                    .on_action(cx.listener(|this, _: &MoveTaskTo, window, cx| {
                        this.open_task_menu(TaskPage::Move, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &TaskMenu, window, cx| {
                        this.open_task_menu(TaskPage::Main, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &PickRepeat, window, cx| {
                        // In Activity, R reads the newest changes again.
                        if this.in_activity() {
                            this.feed_input(FeedInput::Refresh, false, window, cx);
                        } else if !this.ui.compact
                            && let Some(task) = this.repeat_target()
                        {
                            this.scroll_into_view(&this.detail_scroll, "repeat-row");
                            this.open_menu(
                                MenuKind::Repeat { task, draft: None },
                                None,
                                Some(hints::Stop::List),
                                window,
                                cx,
                            );
                            this.highlight_first();
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PickReminder, window, cx| {
                        if !this.ui.compact && !this.in_activity() {
                            this.pick_reminder(hints::Stop::List, window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PickTags, window, cx| {
                        if !this.ui.compact {
                            this.pick_tags(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &AddLinkKey, window, cx| {
                        if !this.ui.compact
                            && let Some(task) = this.ui.task
                        {
                            this.start_link(task, window, cx);
                        }
                    }))
                    .children(self.outgoing_list(swap.outgoing, window, cx))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .opacity(swap.incoming * self.board.opacity(self.frame))
                            .child(body)
                            // Overlay in the task rows' existing right padding.
                            .when(has_rows, |d| {
                                d.child(self.rows_bar.for_list(&self.rows, &p, self.animate))
                            }),
                    )
                    .children(self.email_drop_target(cx)),
            )
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
            .size(px(desktop::SUNRISE_MARK))
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
        self.motion.row_is_visible(id, self.frame)
    }

    fn row_can_act(&self, room: RoomKey, id: TaskId) -> bool {
        self.shell.has_row_in(room, id) && self.row_is_visible(id)
    }

    fn task_is_visible(&self, id: TaskId) -> bool {
        if self.on_timeline() {
            self.timeline_visible(id) && self.row_is_visible(id)
        } else if self.on_board() {
            self.shell
                .room
                .board
                .as_ref()
                .is_some_and(|b| b.columns.iter().flatten().any(|r| r.id == id))
                && self.row_is_visible(id)
        } else {
            self.shell
                .reachable(id, || self.row_is_visible(id), |c| self.handover_entered(c))
        }
    }

    /// The tasks J and K walk, and Space can reach: only ones that can be seen (`Shell::nav_tasks_shown`).
    fn nav_tasks(&self) -> Vec<TaskId> {
        let mut tasks = if self.on_timeline() {
            self.timeline_ids()
        } else if self.on_board() {
            self.board_nav_ids()
        } else {
            self.shell.nav_tasks_shown(|c| self.handover_entered(c))
        };
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
        window: &Window,
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
        let bulk = |id| live && self.multi.contains(id);
        let cursor =
            |id| live && self.multi.cursor() == Some(id) && self.list_focus.is_focused(window);
        let pick = |id: TaskId| {
            cx.listener(
                move |this: &mut Self, click: &gpui::ClickEvent, window, cx| {
                    window.focus(&this.list_focus, cx);
                    if this.task_is_visible(id) {
                        this.click_task(id, click.modifiers(), cx);
                    }
                },
            )
        };
        // A hand-over task completed from its box (or with Space) plays the row's choreography in its
        // place, the same `RowLook` as a list row's: the disc fills, the title dims and the
        // strike-through draws, then the line fades and closes.
        let compact = |task: &TaskRow| {
            let when = task.due.text().cloned().unwrap_or_default();
            let look = live.then(|| self.motion.row(task.id, self.frame)).flatten();
            let height = component::ICON_BUTTON + space::S1;
            let bulk = bulk(task.id);
            let selected = selected == Some(task.id) || bulk;
            let colors = self.palette.for_task_row(bulk);
            self.task_selection(
                div().id(SharedString::from(format!("next-{}", task.id))),
                selected,
                bulk,
                cursor(task.id),
            )
            .when(interactive && !selected, |el| el.hover(|s| s.bg(p.hover)))
            .when(interactive, |el| el.on_click(pick(task.id)))
            .flex_none()
            .mx(px(desktop::LIST_PAD_X))
            .h(px(height * look.map_or(1., |l| l.height)))
            .when_some(look, |el, l| el.overflow_hidden().opacity(l.opacity))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .border_t_1()
            .border_color(p.line)
            .child(self.handover_check(task, look, interactive, bulk, cx))
            .child(Self::struck_title(
                &colors,
                ty::BODY,
                &task.title,
                look.unwrap_or(RowLook::rest(task.done)),
            ))
            .child(kit::chip(t, &task.list, false))
            .child(div().flex_1())
            .child(div().style(ty::DUE).text_color(p.ink_2).child(when))
        };
        let first = &next.first;
        let mark = self.sunrise_mark();
        // Tomorrow's first task plays the same choreography as every other line (Space completes it):
        // its title dims and is struck through, then the card fades and closes its height, so what
        // follows rises into its place before the next task takes it.
        let card_look = live
            .then(|| self.motion.row(first.id, self.frame))
            .flatten();
        let (card_paint, card_height) = card_paint(card_look, first.done);
        // It closes from below, as a list row does in its slot: the card keeps its height inside a clip.
        let card = self
            .task_selection(
                div().id("next-first"),
                selected == Some(first.id) || bulk(first.id),
                bulk(first.id),
                cursor(first.id),
            )
            .when(interactive, |el| el.on_click(pick(first.id)))
            .flex_none()
            .h(px(desktop::SUNRISE_MARK))
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
                    .child(Self::struck_title(
                        &self.palette.for_task_row(bulk(first.id)),
                        ty::NEXT_TITLE,
                        &first.title,
                        card_paint,
                    ))
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
        let card = div()
            .flex_none()
            .mx(px(desktop::LIST_PAD_X))
            .h(px(card_height))
            .when(card_paint.height < 1., |el| el.overflow_hidden())
            .when(card_paint.opacity < 1., |el| el.opacity(card_paint.opacity))
            .child(card);
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

    /// A hand-over task's box. It completes the task (as on Android), with the same guards as a row's:
    /// only in the live room, and only once the task can be reached (it has begun to enter as dusk
    /// falls). Clicking it again part way reverses the completion.
    fn handover_check(
        &self,
        task: &TaskRow,
        look: Option<RowLook>,
        interactive: bool,
        bulk: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let p = &self.palette.for_task_row(bulk);
        let size = component::CHECKBOX - space::S1 / 2.;
        let mark = match look {
            Some(l) => kit::checkbox_at(p, false, bulk, size, (l.fill, l.scale, l.tick)),
            None if task.done => kit::checked(p, bulk, size),
            None => kit::checkbox(p, false, size),
        };
        let (id, room) = (task.id, self.shell.room.key);
        div()
            .id(SharedString::from(format!("next-check-{id}")))
            .flex_none()
            .child(mark)
            .when(interactive, |el| {
                el.on_click(
                    cx.listener(move |this, click: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        if this.shell.room.key == room && this.task_is_visible(id) {
                            window.focus(&this.list_focus, cx);
                            if !this.modified_task_click(id, click.modifiers(), cx) {
                                this.toggle_complete(id, cx);
                            }
                        }
                    }),
                )
            })
    }

    /// Where the keyboard is, for the status bar's hints.
    fn focus_now(&self, window: &Window, cx: &App) -> hints::Focus {
        use hints::Focus;
        let on = |h: &FocusHandle| h.is_focused(window);
        if on(&self.list_focus) {
            Focus::List
        } else if self.splitters.has_focus(window) {
            Focus::Splitter
        } else if self.ai.focus_in(window, cx) {
            Focus::Notes
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
        } else if on(&self.edit.link.read(cx).focus_handle) {
            Focus::Link
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
        } else if on(&self.menu_focus) {
            Focus::Menu
        } else if on(&self.activity_filter_focus)
            || on(&self.activity_refresh_focus)
            || on(&self.activity_more_focus)
            || on(&self.attachment_ui.add)
            || on(&self.dismiss_update_focus)
            || self.bulk_focus.iter().any(on)
            || self.attachment_ui.rows.values().any(on)
        {
            Focus::Button
        } else if on(&self.room_menu_focus)
            || on(&self.repeat_focus)
            || on(&self.due_focus)
            || on(&self.reminder_focus)
            || on(&self.detail_my_day_focus)
            || on(&self.detail_important_focus)
            || on(&self.account_focus)
            || on(&self.update_focus)
            || on(&self.sync_focus)
            || on(&self.new_focus)
            || on(&self.workflow.status)
            || on(&self.workflow.starts)
            || on(&self.workflow.quick)
            || on(&self.workflow.clear)
            || self.workflow.view.iter().any(on)
            || self.timeline.focus.iter().any(on)
            || self.quick_focus.button_focused(window)
        {
            Focus::Control
        } else {
            Focus::Elsewhere
        }
    }

    fn status_bar(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        // Activity counts the changes it shows, and today's among them; the rooms their tasks.
        let counts = match self.shell.room.status {
            _ if self.shell.room.key == RoomKey::Trash => {
                todo_client::trash::status(&self.shell.room.trash, &self.store.now().local())
            }
            Some(counts) if !self.in_activity() => counts,
            _ => {
                let (shown, today) = model::activity::counts(&self.activity, &self.store.now());
                StatusCounts::activity(shown, today)
            }
        };
        let counts = if self.on_timeline() {
            let (scheduled, unscheduled, _) = self.timeline_counts();
            let mut items = vec![(scheduled + unscheduled, "open"), (scheduled, "scheduled")];
            if !matches!(self.ui.room, RoomKey::Smart(model::Smart::Planned)) {
                items.push((unscheduled, "unscheduled"));
            }
            items
        } else {
            vec![counts.first, counts.second]
        };
        let mut keys = hints::key_hints(hints::HintState {
            focus: self.focus_now(window, cx),
            rows: !self.nav_tasks().is_empty(),
            selected: self
                .shell
                .detail
                .as_ref()
                .map(|d| hints::Selected { done: d.done }),
            reorder: matches!(self.ui.room, RoomKey::List(_)),
            undo: self.last_undo.is_some()
                || (!self.animate && self.toast.look(self.frame).is_some_and(|t| t.active)),
            activity: self.activity_hints(),
        });
        if self.list_focus.is_focused(window) && self.on_board() {
            keys = vec![
                ("H L", "columns"),
                ("J K", "cards"),
                ("V", "view"),
                ("N", "add"),
            ];
            if let Some(task) = self.board_task() {
                let done = self.task_stage(task) == Some(todo_core::query::QueryStatus::Done);
                keys.extend([
                    ("Alt ←/→", "move status"),
                    ("S", "status"),
                    ("Space", if done { "reopen" } else { "done" }),
                ]);
            }
            if self.last_undo.is_some() {
                keys.push(("Ctrl Z", "undo"));
            }
        } else if self.list_focus.is_focused(window) && self.on_timeline() {
            keys = vec![
                ("Alt ←/→", "shift ±1d"),
                ("[ ]", "resize"),
                ("0", "today"),
                ("V", "view"),
                ("D / ⇧D", "dates"),
            ];
            if self.last_undo.is_some() {
                keys.push(("Ctrl Z", "undo"));
            }
        } else if self.list_focus.is_focused(window) {
            if self.shell.room.features.workflow || self.shell.room.features.timeline {
                keys.insert(0, ("V", "view"));
            }
            if let Some(task) = self.ui.task {
                if self.task_stage(task).is_some() {
                    keys.insert(0, ("S", "status"));
                }
                if self.task_has_start(task) {
                    keys.insert(0, ("⇧D", "start date"));
                }
            }
        }
        if !self.on_timeline() && self.list_focus.is_focused(window) && !self.nav_tasks().is_empty()
        {
            if self.multi.cursor().is_some() {
                keys.insert(0, ("Esc", "clear selection"));
            }
            keys.push(("Ctrl A", "select all"));
            keys.push(("Shift ↑ ↓", "select range"));
        }
        if self.shell.room.key == RoomKey::Trash {
            keys.retain(|(key, _)| *key != "Ctrl K");
            if self.list_focus.is_focused(window)
                && self.shell.room.trash.iter().any(|r| r.blocked.is_none())
            {
                keys.insert(0, ("↑ ↓", "select"));
                if self.trash_selected.is_some() {
                    keys.insert(1, ("Enter", "restore"));
                }
            }
        }
        if self.width == Width::Wide && !self.ai.modal() {
            keys.insert(0, ("Ctrl J", "Ask AI"));
        }
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
        let right: SharedString =
            self.notice
                .as_ref()
                .map(|n| n.text.clone())
                .unwrap_or_else(|| {
                    self.ai_destination()
                        .unwrap_or_else(|| "Ctrl N adds a task".into())
                });
        // The hints take what the counts and the notice leave, and give way when there isn't enough.
        let figures = ty::COUNT.with_weight(500);
        let left = counts
            .iter()
            .map(|(count, label)| {
                kit::text_width(window, figures, &count.to_string())
                    + kit::text_width(window, ty::COUNT, label)
                    + component::CHIP_GAP
                    + space::S4
            })
            .sum::<f32>();
        let notice = kit::text_width(window, ty::COUNT, &right)
            + if folder.is_some() {
                space::S2 + kit::text_width(window, ty::COUNT, "Open folder")
            } else {
                0.
            };
        let room = f32::from(window.viewport_size().width)
            - 2. * space::S4
            - left
            - notice
            - 2. * space::S6;
        let keys = hints::fit(keys, room, space::S4 + 2., |(key, what)| {
            kit::kbd_width(window, key)
                + component::CHIP_GAP
                + kit::text_width(window, ty::META, what)
        });
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
                    .children(counts.into_iter().map(|(count, label)| {
                        div()
                            .flex()
                            .gap(px(component::CHIP_GAP))
                            .child(strong(count.to_string()))
                            .child(label)
                    })),
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

/// How the finished day's featured card (tomorrow's first task) paints: the row look a list row would
/// have (at rest when nothing plays), and its height, the sunrise mark's, closing with the look.
fn card_paint(look: Option<RowLook>, done: bool) -> (RowLook, f32) {
    let l = look.unwrap_or(RowLook::rest(done));
    (l, desktop::SUNRISE_MARK * l.height)
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

impl ShellView {
    /// The focus handle of a Tab stop.
    fn stop_handle(&self, stop: hints::Stop) -> &FocusHandle {
        use hints::Stop;
        match stop {
            Stop::SidebarSplitter => self.splitters.focus(Pane::Sidebar),
            Stop::List => &self.list_focus,
            Stop::Undo => &self.undo_focus,
            Stop::Bulk(index) => &self.bulk_focus[index],
            Stop::RoomMenu => &self.room_menu_focus,
            Stop::View(i) => &self.workflow.view[i],
            Stop::Timeline(i) => &self.timeline.focus[i],
            Stop::QuickStatus => &self.workflow.quick,
            Stop::QuickRemoveStatus => &self.workflow.clear,
            Stop::Status => &self.workflow.status,
            Stop::Starts => &self.workflow.starts,
            Stop::QuickInput => &self.quick_focus.input,
            Stop::QuickDue => &self.quick_focus.date,
            Stop::QuickPriority => &self.quick_focus.priority,
            Stop::QuickRemoveDate => &self.quick_focus.remove_date,
            Stop::QuickRemovePriority => &self.quick_focus.remove_priority,
            Stop::ActivityFilter => &self.activity_filter_focus,
            Stop::ActivityRefresh => &self.activity_refresh_focus,
            Stop::ActivityMore => &self.activity_more_focus,
            Stop::Due => &self.due_focus,
            Stop::Repeat => &self.repeat_focus,
            Stop::Reminder => &self.reminder_focus,
            Stop::DetailMyDay => &self.detail_my_day_focus,
            Stop::DetailImportant => &self.detail_important_focus,
            Stop::Attachment(FileControl::Add) => &self.attachment_ui.add,
            Stop::Attachment(control) => self
                .attachment_ui
                .rows
                .get(&control)
                .unwrap_or(&self.attachment_ui.add),
            Stop::DetailSplitter => self.splitters.focus(Pane::Detail),
            Stop::AssistantSplitter => self.splitters.focus(Pane::Ai),
            Stop::UpdateIndicator => &self.update_focus,
            Stop::DismissUpdate => &self.dismiss_update_focus,
            Stop::New => &self.new_focus,
            Stop::SyncFailures => &self.sync_focus,
            Stop::Account => &self.account_focus,
        }
    }

    fn repeat_target(&self) -> Option<TaskId> {
        repeat_target(self.detail_placement(), self.ui.task, |id| {
            self.task_is_visible(id)
        })
    }

    /// Whether a Tab stop is there now: Undo while its toast is up, the room's button in a room with a
    /// menu, the detail divider while the docked detail shows it. The others always are.
    fn has_stop(&self, stop: hints::Stop) -> bool {
        use hints::Stop;
        match stop {
            Stop::Undo => {
                !self.toast_without_undo
                    && self.toast.look(self.frame).is_some_and(|look| look.active)
                    && (self.last_undo.is_some() || !self.animate)
            }
            Stop::Bulk(index) => self.bulk_stop(index),
            Stop::RoomMenu => self.has_room_menu(),
            Stop::View(i) => {
                (self.shell.room.features.workflow || self.shell.room.features.timeline)
                    && model::workflow::available(
                        self.shell.room.features,
                        model::workflow::VIEWS[i],
                    )
            }
            Stop::Timeline(i) => self.timeline_control_visible(i),
            Stop::Status => self
                .repeat_target()
                .and_then(|id| self.task_stage(id))
                .is_some(),
            Stop::Starts => self
                .repeat_target()
                .is_some_and(|id| self.task_has_start(id)),
            Stop::QuickStatus => {
                self.shell.room.features.workflow && !self.on_board() && !self.on_timeline()
            }
            Stop::QuickRemoveStatus => {
                self.has_stop(Stop::QuickStatus)
                    && self.quick_stage() != todo_core::query::QueryStatus::Todo
            }
            Stop::QuickInput | Stop::QuickDue | Stop::QuickPriority => {
                !self.in_activity()
                    && self.ui.room != RoomKey::Trash
                    && !self.on_timeline()
                    && (!self.on_board() || self.board_composer_open())
            }
            Stop::QuickRemoveDate => {
                self.has_stop(Stop::QuickInput)
                    && self.quick_date_label(&self.quick_preview()).is_some()
            }
            Stop::QuickRemovePriority => {
                self.has_stop(Stop::QuickInput)
                    && self.quick_preview().priority.unwrap_or_default() != Priority::None
            }
            Stop::ActivityFilter | Stop::ActivityRefresh | Stop::ActivityMore => {
                self.has_activity_stop(stop)
            }
            Stop::Due
            | Stop::Repeat
            | Stop::Reminder
            | Stop::DetailMyDay
            | Stop::DetailImportant
            | Stop::Attachment(FileControl::Add) => self.repeat_target().is_some(),
            Stop::Attachment(control) => self.attachment_ui.rows.contains_key(&control),
            Stop::DetailSplitter => self.show_detail_splitter(),
            Stop::AssistantSplitter => self.ai_docked(),
            Stop::SyncFailures => self.store.sync().failed > 0,
            Stop::UpdateIndicator | Stop::DismissUpdate => {
                self.update_snapshot().indicator_version().is_some()
            }
            Stop::SidebarSplitter | Stop::List | Stop::New | Stop::Account => true,
        }
    }

    /// The Tab stop the keyboard is on, if it's on one (not a text field or an open menu).
    fn stop_at(&self, window: &Window) -> Option<hints::Stop> {
        hints::TAB_ORDER
            .into_iter()
            .find(|&stop| self.stop_handle(stop).is_focused(window))
            .or_else(|| {
                self.attachment_ui
                    .rows
                    .iter()
                    .find_map(|(&control, handle)| {
                        handle
                            .is_focused(window)
                            .then_some(hints::Stop::Attachment(control))
                    })
            })
    }

    /// Moves the keyboard to a Tab stop.
    fn focus_stop(&mut self, stop: hints::Stop, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.stop_handle(stop).clone();
        window.focus(&handle, cx);
        // Reveal stops that can scroll out of the detail pane or the Activity feed.
        if let hints::Stop::Attachment(control) = stop {
            self.reveal_attachment(control);
        } else if let Some((pane, key)) = hints::reveal(stop) {
            let handle = match pane {
                hints::ScrollPane::Detail => &self.detail_scroll,
                hints::ScrollPane::Feed => &self.activity_scroll,
            };
            self.scroll_into_view(handle, key);
        }
        cx.notify();
    }

    /// The one Tab handler: Tab and Shift+Tab go round the stops in `hints::TAB_ORDER` that are there.
    /// Held or modified Tab is left alone, and so is Tab off the stops: a text field keeps it, and an
    /// open menu has already taken it (it closes).
    fn tab_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pairing_panel.is_some()
            || self.ai.modal()
            || self.ai.focus_in(window, cx)
            || !navigation_tab(e)
        {
            return;
        }
        let forward = !e.keystroke.modifiers.shift;
        let files = self
            .shell
            .detail
            .as_ref()
            .into_iter()
            .flat_map(|d| file_controls(&d.attachments));
        let to = hints::tab_target(self.stop_at(window), forward, |s| self.has_stop(s), files);
        if let Some(to) = to {
            cx.stop_propagation();
            self.focus_stop(to, window, cx);
        }
    }

    /// Counts every mouse press in the window for `menu_press`, from a window-level capture listener:
    /// unlike an element's, it runs wherever the press lands, even on a pane that occludes the shell,
    /// and before any menu's outside-press handler (the menus paint later, deferred).
    fn press_counter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        shell.update(cx, |this, _| this.menu_press.press()).ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ai_width_changed(window);
        self.resize(Width::of(f32::from(window.viewport_size().width)));
        self.activity_read_ahead(cx);
        self.drive_motion(window, cx);
        self.drive_ai_flash(window);
        self.drive_email_drop(window, cx);
        self.drive_task_drag(window);
        self.drive_board(window);
        self.drive_timeline(window);
        self.board.clear_geometry();
        self.row_bounds.borrow_mut().clear();
        self.task_toggles
            .retain(|_, a| self.frame.saturating_sub(a.start) < a.span);
        if !self.task_toggles.is_empty() {
            window.request_animation_frame();
        }
        self.keep_focus(window, cx);
        if self.compact.is_some() {
            return self.compact_shell(window, cx).into_any_element();
        }
        self.drive_splitters(window, cx);
        self.sync_attachment_focus(window, cx);
        let p = self.palette;
        // Activity has no tasks to add: its toolbar takes the quick-add's place.
        let quick_add = if self.in_activity() {
            self.activity_toolbar(window, cx)
        } else if self.shell.room.key == RoomKey::Trash {
            div()
                .px(px(desktop::LIST_PAD_X))
                .py(px(space::S3))
                .style(ty::BODY)
                .text_color(p.ink_2)
                .child(todo_client::trash::DESCRIPTION)
        } else if self.on_board() || self.on_timeline() {
            div()
        } else {
            self.quick_add_field(window, cx)
        };
        let detail = self.docked().then(|| self.detail_pane(window, cx));
        let overlay = self.detail_overlay(window, cx);
        let footer = self.list_footer(overlay.is_none(), window, cx);
        let room_body = if self.on_timeline() {
            self.timeline_view(window, cx)
        } else if self.on_board() {
            self.board_view(window, cx)
        } else {
            self.list(window, cx)
        };
        div()
            .id("shell")
            .when(self.pairing_panel.is_none() && !self.ai.modal(), |shell| {
                shell.key_context("Shell")
            })
            .on_key_down(cx.listener(|this, e, window, cx| this.shell_ai_key(e, window, cx)))
            .on_key_down(cx.listener(|this, e, window, cx| this.tab_key(e, window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleAiPanel, window, cx| this.toggle_ai(window, cx)),
            )
            .on_action(cx.listener(|this, _: &UndoTask, window, cx| {
                let focus = this.focus_now(window, cx);
                if focus.allows_task_undo() {
                    // Like any shortcut, it closes an open menu first (the keyboard goes back to its
                    // button), so a confirmation never stays up over the changed state.
                    if focus == hints::Focus::Menu {
                        this.close_menu(true, window, cx);
                    }
                    this.request_undo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenActivity, window, cx| {
                // Never while typing: the field keeps the keyboard and nothing changes.
                if this.focus_now(window, cx).allows_task_undo() {
                    if this.focus_now(window, cx) == hints::Focus::Menu {
                        this.close_menu(false, window, cx);
                    }
                    this.select_room(RoomKey::Activity(None), cx);
                    window.focus(&this.list_focus, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                if this.shell.room.key == RoomKey::Trash {
                    return;
                }
                let handle = this.filter.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusQuickAdd, window, cx| {
                // History has no quick-add: adding a task goes to My Day's.
                if this.shell.room.key.is_history() {
                    this.select_room(RoomKey::default(), cx);
                }
                if this.on_timeline() {
                    this.switch_view(todo_core::model::ListView::List, window, cx);
                }
                if this.on_board() {
                    this.board_add(window, cx);
                } else {
                    let handle = this.quick_add.read(cx).focus_handle.clone();
                    window.focus(&handle, cx);
                    cx.notify();
                }
            }))
            // A click that no field takes (the sidebar, the header, a button) leaves the keyboard on the
            // task list, so its keys keep working.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.pairing_panel.is_none()
                        && !this.ai.modal()
                        && !window.default_prevented()
                    {
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
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_t_1()
                    .border_color(p.line)
                    .child(self.sidebar(window, cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(p.pane)
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .child(self.header(window, cx))
                                    .child(quick_add)
                                    .child(room_body)
                                    .children(overlay.into_iter().flatten()),
                            )
                            .children(self.bulk_bar(window, cx))
                            .child(footer),
                    )
                    .children(detail)
                    .child(self.pane_handle(Pane::Sidebar, window, cx))
                    .when(self.show_detail_splitter(), |body| {
                        body.child(self.pane_handle(Pane::Detail, window, cx))
                    })
                    .children(self.ai_pane(window, cx))
                    .when(self.ai_docked(), |body| {
                        body.child(self.pane_handle(Pane::Ai, window, cx))
                    }),
            )
            .child(self.status_bar(window, cx))
            .child(self.pane_capture(cx))
            .when(self.dragging_pane(), |shell| {
                shell.child(div().absolute().inset_0().occlude().cursor_col_resize())
            })
            .child(self.press_counter(cx))
            .child(self.drag_events(cx))
            .child(self.board_capture(cx))
            .child(self.timeline_capture(cx))
            .children(self.timeline_preview(window))
            .children(self.drag_preview(window))
            .children(self.board_preview(window, cx))
            .children(self.confirmation_scrim())
            .children(self.pairing_overlay())
            .children(self.ai_settings_overlay(window, cx))
            // Context menus open at the pointer; the others hang from their buttons.
            .children(self.popover(
                |k| {
                    matches!(
                        k,
                        MenuKind::List { .. }
                            | MenuKind::Smart(_)
                            | MenuKind::Collection { .. }
                            | MenuKind::Task { .. }
                            | MenuKind::Status { at_row: true, .. }
                            | MenuKind::BulkMove
                            | MenuKind::BulkStatus
                    ) || matches!(k, MenuKind::Date(c) if c.timeline.is_some())
                },
                cx,
            ))
            .into_any_element()
    }
}

/// The task list's keys that act on the selected task, read from the raw key event rather than bound,
/// so a held key's repeats can be told apart: `(key, act)`, where `act` is false for a repeat, which is
/// consumed without acting. Holding Space never flips a task back and forth, and holding Delete never
/// deletes the rows that take the selection one after another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKey {
    Toggle,
    Delete,
}

fn list_key(e: &gpui::KeyDownEvent) -> Option<(ListKey, bool)> {
    if e.keystroke.modifiers.modified() {
        return None;
    }
    let key = match e.keystroke.key.as_str() {
        "space" => ListKey::Toggle,
        "delete" => ListKey::Delete,
        _ => return None,
    };
    Some((key, !e.is_held))
}

/// Both tab directions navigate without stealing system or application shortcuts.
fn navigation_tab(event: &gpui::KeyDownEvent) -> bool {
    let key = &event.keystroke;
    key.key == "tab"
        && !event.is_held
        && !key.modifiers.control
        && !key.modifiers.alt
        && !key.modifiers.platform
        && !key.modifiers.function
}

/// R, Shift+R and the Repeat and Reminder Tab stops have the same target: the open task. A stale
/// selection behind a hidden pane is inert.
fn repeat_target(
    pane: DetailPane,
    selected: Option<TaskId>,
    visible: impl Fn(TaskId) -> bool,
) -> Option<TaskId> {
    selected.filter(|&id| pane != DetailPane::Hidden && visible(id))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{RowLook, card_paint};
    use crate::fixtures;
    use crate::model::motion::{Motion, Pace, Release};
    use crate::theme::{desktop, motion as m};

    #[test]
    fn r_has_no_target_behind_a_hidden_pane() {
        use super::{DetailPane, repeat_target};
        let selected = fixtures::my_day().ui.task;
        assert!(selected.is_some());
        assert_eq!(repeat_target(DetailPane::Hidden, selected, |_| true), None);
        assert_eq!(repeat_target(DetailPane::Docked, selected, |_| false), None);
        assert_eq!(
            repeat_target(DetailPane::Overlay, selected, |_| true),
            selected
        );
    }

    fn ms(n: u32) -> Duration {
        Duration::from_millis(u64::from(n))
    }

    #[test]
    fn the_featured_hand_over_card_plays_the_whole_completion_then_gives_way() {
        let f = fixtures::handover_complete(0);
        let next = f.shell().room.handover().cloned().expect("hands over");
        let first = next.first.id;
        // Held while its completion plays: still the featured card, drawn done.
        assert!(next.first.done);
        let mut motion = Motion::default();
        motion.complete(first, Duration::ZERO, Pace::FULL);
        let at = |t| card_paint(motion.row(first, ms(t)), true);

        // Part way through the tick: the title is dimming and the strike-through drawing, at full height.
        let (l, height) = at(m::CHECK_TICK_DELAY + m::CHECK_TICK / 2);
        assert!(l.dim > 0. && l.dim < 1., "{l:?}");
        assert!(l.strike > 0. && l.strike < 1., "{l:?}");
        assert_eq!((height, l.opacity), (desktop::SUNRISE_MARK, 1.));
        // Half way through the collapse: struck and dimmed, fading and closing its height.
        let (l, height) =
            at(m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD + m::ROW_COLLAPSE / 2);
        assert_eq!((l.dim, l.strike), (1., 1.));
        assert!(height > 0. && height < desktop::SUNRISE_MARK, "{height}");
        assert!(l.opacity > 0. && l.opacity < 1., "{l:?}");

        // Released once it has gone: nothing of it is left, and the next task takes the card.
        let end = ms(m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD + m::ROW_COLLAPSE);
        let (l, height) = card_paint(motion.row(first, end - ms(1)), true);
        assert!(height < 1. && l.opacity < 0.1, "{height} {l:?}");
        assert!(motion.advance(end).contains(&Release::Completed(first)));
        assert_eq!(motion.holding().count(), 0);
        let mut f = f;
        f.ui.holding.clear();
        let after = f.shell();
        let after = after.room.handover().expect("still hands over");
        assert_eq!(
            after.first.id,
            next.tasks().nth(1).expect("a second task").id
        );
        assert!(after.tasks().all(|t| t.id != first));
        // At rest, a card is the sunrise mark's height and undimmed.
        assert_eq!(
            card_paint(None, false),
            (RowLook::rest(false), desktop::SUNRISE_MARK)
        );
    }

    #[test]
    fn holding_delete_deletes_only_the_task_selected_when_it_was_pressed() {
        use super::{ListKey, list_key};
        use crate::model::commands;
        use gpui::{KeyDownEvent, Keystroke};
        let press = |text, is_held| KeyDownEvent {
            keystroke: Keystroke::parse(text).expect("keystroke"),
            is_held,
            prefer_character_input: false,
        };
        let f = fixtures::my_day();
        let store = crate::store::Store::fixture(&f);
        let mut ui = f.ui.clone();
        let first = ui.task.expect("selected");
        // Windows sends the press, then repeats while the key is held. Each delete hands the selection
        // to the next row, so a repeat that acted would delete that one too.
        for held in [false, true, true, true] {
            let Some((ListKey::Delete, act)) = list_key(&press("delete", held)) else {
                panic!("Delete is the list's");
            };
            if !act {
                continue;
            }
            let id = ui.task.expect("a selection");
            let shell = crate::model::derive(&store.view(), &ui, &store.sync(), &store.now());
            let rows: Vec<_> = shell.rows().map(|r| r.id).collect();
            let at = rows.iter().position(|&r| r == id).expect("shown");
            store
                .submit(commands::delete_task(&store.view(), id).expect("delete"))
                .expect("deleted");
            ui.task = rows.get(at + 1).copied();
        }
        let state = store.view();
        let deleted: Vec<_> = (state.tasks.values())
            .filter(|t| t.deleted.is_some() && !f.state.tasks[&t.id].deleted.is_some())
            .map(|t| t.id)
            .collect();
        assert_eq!(deleted, [first], "only the first press deleted");
        assert_ne!(ui.task, Some(first), "the selection moved on");
        assert!(state.is_live(ui.task.expect("selected").into()));
        // Space works the same way; modified keys aren't the list's.
        assert_eq!(
            list_key(&press("space", true)),
            Some((ListKey::Toggle, false))
        );
        assert_eq!(
            list_key(&press("space", false)),
            Some((ListKey::Toggle, true))
        );
        assert_eq!(list_key(&press("shift-delete", false)), None);
        assert_eq!(list_key(&press("e", false)), None);
    }

    #[test]
    fn tab_navigation_does_not_steal_modified_or_repeated_tab() {
        use gpui::{KeyDownEvent, Keystroke};
        let key = |text| KeyDownEvent {
            keystroke: Keystroke::parse(text).expect("keystroke"),
            is_held: false,
            prefer_character_input: false,
        };
        assert!(super::navigation_tab(&key("tab")));
        assert!(super::navigation_tab(&key("shift-tab")));
        assert!(!super::navigation_tab(&key("ctrl-tab")));
        assert!(!super::navigation_tab(&key("alt-tab")));
        assert!(!super::navigation_tab(&KeyDownEvent {
            is_held: true,
            ..key("tab")
        }));
    }

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
