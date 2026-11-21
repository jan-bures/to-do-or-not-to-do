//! The main window: titlebar, sidebar, the room pane under its sky header, the detail pane and the status
//! bar. It draws a `model::Shell`, derived from the store's state whenever the data or the window's own
//! state changes, and turns clicks and keys into commands for the store.

mod detail;
mod sidebar;

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, AsyncApp, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, MouseButton, SharedString, Stateful,
    UniformListScrollHandle, WeakEntity, Window, WindowControlArea, actions, div, prelude::*, px,
    uniform_list,
};
use todo_client::Connection;
use todo_core::ids::{CollectionId, TaskId};

use super::input::{InputColors, InputEvent, TextInput};
use super::kit;
use super::landscape::{LandscapeCache, Sky, landscape};
use crate::model::{
    self, Chips, Due, Eyebrow, RoomKey, Shell, TaskRow, Tomorrow, UiState, commands, quick_add,
};
use crate::store::Store;
use crate::theme::{
    self, Icon, Palette, TextStyled, Theme, component, desktop, elevation, motion, radius, space,
    ty,
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
        ToggleTask,
        EditTask,
        EditDue,
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
        KeyBinding::new("alt-up", MoveTaskUp, list),
        KeyBinding::new("alt-down", MoveTaskDown, list),
        KeyBinding::new("j", NextTask, list),
        KeyBinding::new("down", NextTask, list),
        KeyBinding::new("k", PreviousTask, list),
        KeyBinding::new("up", PreviousTask, list),
        KeyBinding::new("space", ToggleTask, list),
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

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct ShellView {
    store: Rc<Store>,
    ui: UiState,
    shell: Shell,
    theme: Theme,
    palette: Palette,
    landscape: LandscapeCache,
    quick_add: Entity<TextInput>,
    focus: FocusHandle,
    list_focus: FocusHandle,
    rows: UniformListScrollHandle,
    menu: Option<Menu>,
    inline: Option<Inline>,
    /// The filter typed into the title bar.
    filter: Entity<TextInput>,
    edit: Editors,
    /// Completed tasks fading out of the list (the last part of the completion hold).
    leaving: BTreeSet<TaskId>,
    /// A message for the status bar (a command the domain refused, a failed download).
    notice: Option<Notice>,
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
        let mut this = Self {
            store,
            ui,
            shell,
            theme,
            palette,
            landscape: LandscapeCache::default(),
            quick_add,
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            rows: UniformListScrollHandle::new(),
            menu: None,
            inline: None,
            filter,
            edit,
            leaving: BTreeSet::new(),
            notice: None,
        };
        this.load_detail(cx);
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
        let session = self.store.session();
        // Prune only once the stream is live and caught up: during catch-up after a new or rebuilt
        // cache, a collection from a later page isn't in the replica yet. Stale IDs are harmless
        // meanwhile, since deriving the sidebar only asks whether an ID is collapsed.
        let can_prune =
            session.is_none_or(|s| s.client().status().connection == Connection::Online);
        let state = self.store.view();
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
        if shell.room.hue != self.shell.room.hue || self.palette.theme != self.theme {
            self.palette = theme::palette(self.theme, shell.room.hue);
            let colors = quick_add_colors(&self.palette);
            for input in [&self.quick_add, &self.filter, &self.edit.step] {
                input.update(cx, |i, _| i.colors = colors);
            }
            for area in [&self.edit.title, &self.edit.notes] {
                area.update(cx, |a, _| a.colors = colors);
            }
        }
        if shell.room.quick_add_placeholder != self.shell.room.quick_add_placeholder {
            let placeholder = shell.room.quick_add_placeholder.clone();
            self.quick_add.update(cx, |i, cx| {
                i.placeholder = placeholder;
                cx.notify();
            });
        }
        self.shell = shell;
        self.load_detail(cx);
        cx.notify();
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
        let room_changed = self.ui.room != room;
        let revealed = self.ui.select_room(&self.store.view(), room);
        if room_changed {
            self.rows.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
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
        if self.ui.task != id && !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        self.ui.task = id;
        self.refresh(cx);
        if let Some(i) = self.shell.selected_task {
            self.rows.scroll_to_item(i, gpui::ScrollStrategy::Center);
        }
    }

    /// Completes or reopens a task. A task completed in the open list holds its place for a moment
    /// (the checkbox fills, the title dims and strikes through), fades, and then leaves the list.
    fn toggle_complete(&mut self, id: TaskId, cx: &mut Context<Self>) {
        let state = self.store.view();
        let completing = state.tasks.get(&id).is_some_and(|t| !t.is_done());
        if let Some(cmd) = commands::toggle_complete(&state, id) {
            if self.submit(vec![cmd]) && completing {
                self.ui.holding.insert(id);
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    let ms = |n: u32| Duration::from_millis(u64::from(n));
                    cx.background_executor()
                        .timer(ms(motion::CHECK_TICK_DELAY
                            + motion::CHECK_TICK
                            + motion::COMPLETE_HOLD))
                        .await;
                    let _ = this.update(cx, |v, cx| {
                        v.leaving.insert(id);
                        cx.notify();
                    });
                    cx.background_executor()
                        .timer(ms(motion::ROW_COLLAPSE))
                        .await;
                    let _ = this.update(cx, |v, cx| {
                        v.leaving.remove(&id);
                        v.ui.holding.remove(&id);
                        v.refresh(cx);
                    });
                })
                .detach();
            } else if !completing {
                self.ui.holding.remove(&id);
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

    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.shell.row_count();
        if n == 0 {
            return;
        }
        let next = match self.shell.selected_task {
            None if by > 0 => 0,
            None => n - 1,
            Some(i) => i.saturating_add_signed(by).min(n - 1),
        };
        let id = self.shell.row(next).map(|r| r.id);
        self.select_task(id, cx);
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
            .child(kit::window_controls(p))
    }

    fn eyebrow(&self) -> Div {
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
        match &self.shell.room.eyebrow {
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
        let p = if self.shell.room.finished_day() {
            theme::palette(Theme::Dark, self.shell.room.hue)
        } else {
            self.palette
        };
        (p.ink_2, p.ink_3, p.line_strong)
    }

    /// The accent on the dusk scene: the dark theme's, since the scene is a night scene.
    fn dusk_accent(&self) -> Hsla {
        theme::palette(Theme::Dark, self.shell.room.hue).accent_strong
    }

    fn header(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let finished = room.finished_day();
        let (sky_bg, title_ink, sky) = if finished {
            let dusk = theme::dusk(room.hue);
            let ink = theme::palette(Theme::Dark, room.hue).ink;
            (dusk.top, ink, Sky::Finished(dusk))
        } else {
            (p.sky, p.ink, Sky::Slim(self.palette))
        };
        let (_, ink_3, _) = self.header_inks();
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
                            .child(self.eyebrow())
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S1))
                                    .mr(px(-space::S2))
                                    .child(action(Icon::SlidersHorizontal))
                                    .child(action(Icon::Ellipsis)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(space::S6))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .style(ty::ROOM_TITLE)
                                    .text_color(title_ink)
                                    .child(room.title.clone()),
                            )
                            .child(div().flex_none().child(self.summary())),
                    ),
            )
            // The finished scene's rings reach past the strip; the header ends at its bottom edge.
            .child(div().overflow_hidden().child(landscape(
                &self.landscape,
                sky,
                room.done,
                room.total,
            )))
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
    }

    fn quick_add_field(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        let input = self.quick_add.read(cx);
        let focused = input.focus_handle.is_focused(window);
        let text = input.content.clone();
        let typed = !text.is_empty();
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
            } else {
                p.control
            })
            .bg(p.raised)
            .style(ty::QUICK_ADD)
            .child(kit::icon(Icon::Plus, component::ICON, p.accent_strong))
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

    /// A row, fading out while it leaves after completion.
    fn row_element(&self, i: usize, task: &TaskRow, cx: &mut Context<Self>) -> AnyElement {
        let row = self.row(i, task, cx);
        if self.leaving.contains(&task.id) {
            let fade = Animation::new(Duration::from_millis(u64::from(motion::ROW_COLLAPSE)))
                .with_easing(|t| motion::EXIT.ease(t));
            row.with_animation(("leaving", i), fade, |el, t| el.opacity(1. - t))
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    fn row(&self, i: usize, task: &TaskRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = &self.palette;
        let t = self.theme;
        let selected = self.shell.selected_task == Some(i);
        let overdue = matches!(task.due, Due::Overdue(_));
        let mut meta = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .h(px(component::CHIP))
            .mt(px(desktop::ROW_LINE_GAP))
            .overflow_hidden();
        match self.shell.room.chips {
            Chips::None => {}
            Chips::List => meta = meta.child(kit::chip(t, &task.list, false)),
            Chips::CollectionAndList => meta = meta.child(kit::chip(t, &task.list, true)),
        }
        if let Some((d, n)) = task.steps {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(component::CHIP_GAP))
                    .text_color(p.ink_3)
                    .style(ty::COUNT.with_weight(500))
                    .child(kit::meter(p, d, n))
                    .child(format!("{d}/{n}")),
            );
        }
        for tag in &task.tags {
            meta = meta.child(div().style(ty::TAG).text_color(p.ink_3).child(tag.clone()));
        }
        if task.from_mail {
            meta = meta.child(kit::icon(Icon::Mail, component::ICON_SMALL, p.ink_3));
        }
        let id = task.id;
        let check = if task.done && self.ui.holding.contains(&id) {
            kit::checking(p, selected, desktop::COL_CHECK, ("checking", i)).into_any_element()
        } else if task.done {
            kit::checked(p, selected, desktop::COL_CHECK).into_any_element()
        } else {
            kit::checkbox(p, overdue, desktop::COL_CHECK).into_any_element()
        };
        let check = div()
            .id(SharedString::from(format!("check-{id}")))
            .flex_none()
            .child(check)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_complete(id, cx);
            }));
        let title = div()
            .style(ty::TASK_TITLE)
            .truncate()
            .child(task.title.clone());
        let title = if task.done {
            title.text_color(p.ink_3).line_through()
        } else {
            title.text_color(p.ink)
        };
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
            .child(
                div().w(px(desktop::COL_AGENT)).flex_none().children(
                    task.agent
                        .map(|a| kit::agent_mark(t, a, component::AGENT_MARK)),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("priority-{id}")))
                    .w(px(desktop::COL_PRIORITY))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(kit::priority(p, task.priority))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.cycle_priority(id, cx);
                    })),
            )
            .child(
                div()
                    .w(px(desktop::COL_DUE))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(Self::due(p, &task.due)),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select_task(Some(id), cx);
            }));
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
            row.hover(move |s| s.bg(hover))
        }
    }

    fn list(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let room = &self.shell.room;
        let body: AnyElement = if let (true, Some(next)) = (room.finished_day(), &room.tomorrow) {
            self.tomorrow(next, cx).into_any_element()
        } else if room.finished_day() {
            div()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6 + space::S1))
                .child(
                    div()
                        .style(ty::EMPTY_LINE)
                        .text_color(p.ink)
                        .child("Nothing waiting for tomorrow."),
                )
                .into_any_element()
        } else if self.shell.row_count() == 0 {
            div()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6 + space::S1))
                .child(
                    div()
                        .style(ty::EMPTY_LINE)
                        .text_color(p.ink)
                        .child(room.empty_line.clone()),
                )
                .child(
                    div()
                        .mt(px(space::S2 + 2.))
                        .style(ty::BODY)
                        .text_color(p.ink_3)
                        .child("Add one above, or press Ctrl+Alt+Space from anywhere."),
                )
                .into_any_element()
        } else {
            let count = self.shell.row_count();
            uniform_list(
                "rows",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let rows: Vec<TaskRow> = range
                        .clone()
                        .filter_map(|i| this.shell.row(i).cloned())
                        .collect();
                    range
                        .zip(rows.iter())
                        .map(|(i, r)| this.row_element(i, r, cx))
                        .collect::<Vec<_>>()
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
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_action(cx.listener(|this, _: &NextTask, _, cx| this.move_selection(1, cx)))
                    .on_action(
                        cx.listener(|this, _: &PreviousTask, _, cx| this.move_selection(-1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &ToggleTask, _, cx| {
                        if let Some(id) = this.ui.task {
                            this.toggle_complete(id, cx);
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
                    .on_action(cx.listener(|this, _: &MoveTaskUp, _, cx| this.move_task(-1, cx)))
                    .on_action(cx.listener(|this, _: &MoveTaskDown, _, cx| this.move_task(1, cx)))
                    .child(body),
            )
            .children(completed)
    }

    /// The finished day hands over to tomorrow: its first task with a sunrise mark, then the rest of
    /// tomorrow and the rest of the week. No congratulations; the scene is the reward.
    fn tomorrow(&self, next: &Tomorrow, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let section = |label: SharedString| {
            div()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6))
                .pb(px(space::S2))
                .text_color(p.ink_3)
                .style(ty::CAPS_LABEL)
                .child(label.to_uppercase())
        };
        let compact = |task: &TaskRow| {
            let when = task.due.text().cloned().unwrap_or_default();
            div()
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
        let mark = div()
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
            ));
        let card = div()
            .mx(px(desktop::LIST_PAD_X))
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
            .child(
                // Planning the next day happens in Planned, where tomorrow's tasks are.
                div()
                    .id("plan-next")
                    .child(kit::button(p, format!("Plan {}", next.weekday)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_room(RoomKey::Smart(model::Smart::Planned), cx);
                    })),
            );
        div()
            .flex()
            .flex_col()
            .child(section("When the sun comes up".into()))
            .child(card)
            .when(!next.also.is_empty(), |d| {
                d.child(section(format!("Also on {}", next.weekday).into()))
                    .children(next.also.iter().map(compact))
            })
            .when(!next.week.is_empty(), |d| {
                d.child(section("Rest of the week".into()))
                    .children(next.week.iter().map(compact))
            })
            .when(next.more > 0, |d| {
                d.child(
                    div()
                        .px(px(desktop::LIST_PAD_X))
                        .pt(px(space::S3))
                        .style(ty::COUNT)
                        .text_color(p.ink_3)
                        .child(format!("{} more", next.more)),
                )
            })
    }

    fn status_bar(&self) -> Div {
        let p = &self.palette;
        let (done, _, open) = self.shell.room.summary();
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
                            .child(strong(format!("{done:02}")))
                            .child("done"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S4 + 2.))
                    .style(ty::META)
                    .child(hint("J K", "move"))
                    .child(hint("Space", "complete"))
                    .child(hint("E", "edit"))
                    .child(hint("D", "due")),
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

impl Focusable for ShellView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let finished = self.shell.room.finished_day();
        let quick_add = (!finished).then(|| self.quick_add_field(window, cx));
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
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
            .on_mouse_down(MouseButton::Left, |_, _, _| {})
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
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(p.pane)
                            .child(self.header())
                            // The finished day hands over to tomorrow instead of taking new tasks.
                            .children(quick_add)
                            .child(self.list(cx)),
                    )
                    .child(self.detail_pane(window, cx)),
            )
            .child(self.status_bar())
            .children(self.menu(cx))
    }
}
