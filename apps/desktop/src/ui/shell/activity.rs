//! The Activity room: its toolbar in the quick-add's place (the order, Clear filter, Refresh
//! activity), the changes by day, newest first, each with its Undo, and the end of the feed (Load
//! older activity, or why a read failed). Older changes are read as the feed scrolls, or as the
//! keyboard nears the end of what has been read.
//!
//! The state and rows are `model::activity`'s; the pages, undos and words are shared with the web
//! app (`todo_client::activity`).

use std::collections::HashSet;

use gpui::{
    AnyElement, AsyncApp, ClickEvent, Context, Div, Hsla, KeyDownEvent, Pixels, SharedString,
    Stateful, WeakEntity, Window, div, prelude::*, px,
};
use todo_client::Epoch;
use todo_client::activity::{
    self as feed, ActivityError, ActivityPage, ActivityQuery, Loaded, UndoAction, UndoOutcome,
};

use super::{Notice, ShellView};
use crate::model::activity::{self, ActivityDay, ActivityRow, ActivityState, AgentKey, Mark};
use crate::model::hints::{self, Stop};
use crate::model::{RoomKey, layout};
use crate::theme::{self, Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use crate::ui::kit;

/// The replica's position when a page was asked for.
type Revision = Option<(Option<Epoch>, u64)>;

/// A toolbar button: its Tab stop, icon and label; busy (a refresh being read) and the dot that
/// says there is news.
struct Tool {
    stop: Stop,
    icon: Icon,
    label: &'static str,
    busy: bool,
    dot: bool,
}

impl ShellView {
    /// The agent filter, while the room is Activity.
    pub(super) fn activity_room(&self) -> Option<Option<AgentKey>> {
        match self.shell.room.key {
            RoomKey::Activity(agent) => Some(agent),
            _ => None,
        }
    }

    pub(super) fn in_activity(&self) -> bool {
        self.activity_room().is_some()
    }

    /// Starts the feed for `agent` afresh (nothing read yet), at the top, and reads the newest page.
    pub(super) fn open_activity(&mut self, agent: Option<AgentKey>, cx: &mut Context<Self>) {
        self.activity = ActivityState::new(agent);
        self.activity.epoch = self.store.activity_revision().and_then(|(epoch, _)| epoch);
        self.activity_bar.reset_to_top(&self.activity_scroll);
        self.activity_rows.borrow_mut().clear();
        self.load_activity(None, cx);
    }

    /// Reads the page before `before` (the newest for `None`). A fixture's is read at once; a live
    /// session reads it on the network runtime. Nothing while another read is in flight, except that
    /// a refresh asked for meanwhile follows it.
    pub(super) fn load_activity(&mut self, before: Option<u64>, cx: &mut Context<Self>) {
        let Some(request) = self.activity.feed.begin(before) else {
            cx.notify();
            return;
        };
        let revision = self.store.activity_revision();
        let query = ActivityQuery {
            before,
            limit: Some(activity::PAGE_SIZE),
            agent: self.activity.agent.and_then(|a| a.id()),
        };
        if let Some(result) = self.store.local_activity(&query) {
            self.finish_activity(request, before, revision, result, cx);
            return;
        }
        let Some(session) = self.store.session() else {
            return;
        };
        let read = session.activity(query);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = read.await;
            let _ = this.update(cx, |v, cx| {
                v.finish_activity(request, before, revision, result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_activity(
        &mut self,
        request: u64,
        before: Option<u64>,
        revision: Revision,
        result: Result<ActivityPage, ActivityError>,
        cx: &mut Context<Self>,
    ) {
        let fresh = before.is_none() && result.is_ok();
        let shown: HashSet<u64> = self.activity.feed.entries().map(|e| e.target.seq).collect();
        match self.activity.feed.finish(request, before, result) {
            Loaded::Ignore => return,
            Loaded::Refresh => return self.load_activity(None, cx),
            Loaded::Applied | Loaded::SignIn => {}
        }
        if fresh {
            self.activity.revision = revision;
        }
        self.activity.keep_selection();
        // The rows a page brings in fade in together, never one by one; rows already shown stay.
        if self.animate {
            let (now, pace) = (self.now(), self.pace());
            let new: Vec<u64> = (self.activity.feed.entries())
                .map(|e| e.target.seq)
                .filter(|seq| !shown.contains(seq))
                .collect();
            self.activity.motion.enter(new, now, pace);
        }
        cx.notify();
    }

    /// After the data changed: the undos asked for here that have gone through or been refused say
    /// so, and a replaced log starts the feed again.
    pub(super) fn follow_activity(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.activity_room() else {
            return;
        };
        let store = self.store.clone();
        let refused: HashSet<u64> = self.refused_changes();
        if let Some(said) = self.activity.undos.settle(|c| store.undo_settled(c)) {
            self.notice = Some(if said == feed::UNDONE {
                Notice::info(said)
            } else {
                Notice::error(said)
            });
            if self.animate {
                let (now, pace) = (self.now(), self.pace());
                for seq in self.refused_changes().difference(&refused) {
                    self.activity.motion.note(*seq, now, pace);
                }
            }
        }
        let epoch = store.activity_revision().and_then(|(epoch, _)| epoch);
        if self.activity.epoch.is_some_and(|old| Some(old) != epoch) {
            self.open_activity(agent, cx);
        } else {
            self.activity.epoch = epoch;
        }
    }

    fn refused_changes(&self) -> HashSet<u64> {
        (self.activity.feed.entries())
            .filter(|e| self.activity.undos.refusal(e.target.id).is_some())
            .map(|e| e.target.seq)
            .collect()
    }

    /// Refresh activity: the newest changes again, without moving the feed.
    pub(super) fn refresh_activity(&mut self, cx: &mut Context<Self>) {
        if self.in_activity() {
            self.load_activity(None, cx);
        }
    }

    /// The rows to draw now.
    pub(super) fn activity_days(&self) -> Vec<ActivityDay> {
        let store = &self.store;
        activity::days(&self.activity, &self.store.now(), |entry| {
            store.activity_flags(entry)
        })
    }

    /// Undoes change `seq` (Undo, or Enter on the selected change), unless it can't be undone now.
    pub(super) fn undo_change(&mut self, seq: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.activity.feed.entry(seq).cloned() else {
            return;
        };
        let (confirmed, pending) = self.store.activity_flags(&entry);
        let group = self.activity.undos.group_outcome(entry.group);
        if feed::undo_action(&entry, confirmed, pending, group) != UndoAction::Undo {
            return;
        }
        let outcome = self.store.undo_activity(entry.target);
        let said = self
            .activity
            .undos
            .record(entry.target.id, entry.group, outcome);
        if matches!(outcome, UndoOutcome::Refused(_)) {
            self.notice = Some(Notice::error(said));
            if self.animate {
                let (now, pace) = (self.now(), self.pace());
                self.activity.motion.note(seq, now, pace);
            }
        } else {
            self.notice = Some(Notice::info(said));
        }
        self.refresh(cx);
    }

    /// Selects change `seq` (or none), with the selection's short crossfade.
    pub(super) fn select_change(&mut self, seq: Option<u64>, cx: &mut Context<Self>) {
        let from = self.activity.selected;
        self.activity.selected = seq;
        if self.animate {
            let (now, pace) = (self.now(), self.pace());
            self.activity.motion.select(from, seq, now, pace);
        }
        cx.notify();
    }

    /// Up and Down: the next or previous change, scrolled into view; near the end of what has been
    /// read, the next page is read.
    pub(super) fn activity_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let from = self.activity.selected;
        if self.activity.step(by) {
            if self.animate {
                let (now, pace) = (self.now(), self.pace());
                (self.activity.motion).select(from, self.activity.selected, now, pace);
            }
            self.reveal_change();
        }
        if self.activity.near_end()
            && let Some(older) = self.activity.feed.older()
        {
            self.load_activity(Some(older), cx);
        }
        cx.notify();
    }

    /// Enter on the list: undoes the selected change.
    pub(super) fn activity_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = &e.keystroke;
        if key.modifiers.modified() || !matches!(key.key.as_str(), "enter" | "u") {
            return false;
        }
        if !e.is_held
            && let Some(seq) = self.activity.selected
        {
            self.undo_change(seq, cx);
        }
        true
    }

    /// Scrolls the selected change into view, a small margin clear of the feed's edges.
    fn reveal_change(&self) {
        let Some(seq) = self.activity.selected else {
            return;
        };
        let Some(target) = self.activity_rows.borrow().get(&seq).copied() else {
            return;
        };
        let handle = &self.activity_scroll;
        let view = handle.bounds();
        let offset = handle.offset();
        if let Some(y) = layout::reveal_offset(
            (f32::from(view.top()), f32::from(view.bottom())),
            (f32::from(target.top()), f32::from(target.bottom())),
            f32::from(offset.y),
            f32::from(handle.max_offset().y),
            space::S2,
        ) {
            handle.set_offset(gpui::point(offset.x, px(y)));
        }
    }

    /// While the feed is scrolled close to its end, the next page is read (each frame, so a page too
    /// short to fill the pane reads the next one too).
    pub(super) fn activity_read_ahead(&mut self, cx: &mut Context<Self>) {
        if !self.in_activity() || self.activity.feed.pending().is_some() {
            return;
        }
        let Some(older) = self.activity.feed.older() else {
            return;
        };
        let handle = &self.activity_scroll;
        let viewport = f32::from(handle.bounds().size.height);
        // Not laid out yet: nothing to measure.
        if viewport <= 0. {
            return;
        }
        let left = f32::from(handle.max_offset().y) + f32::from(handle.offset().y);
        if left <= viewport / 4. {
            self.load_activity(Some(older), cx);
        }
    }

    /// Whether one of the room's Tab stops is there now.
    pub(super) fn has_activity_stop(&self, stop: Stop) -> bool {
        match (self.activity_room(), stop) {
            (Some(agent), Stop::ActivityFilter) => agent.is_some(),
            (Some(_), Stop::ActivityRefresh) => true,
            (Some(_), Stop::ActivityMore) => self.more_button().is_some(),
            _ => false,
        }
    }

    /// What the button at the end of the feed does: read the next page, or the failed one again (or
    /// past the change too large to show), with its label.
    fn more_button(&self) -> Option<(Option<u64>, &'static str)> {
        let f = &self.activity.feed;
        if let (Some(error), Some(retry)) = (f.error(), f.retry()) {
            return Some((retry, feed::retry_label(error)));
        }
        f.older().map(|older| (Some(older), feed::LOAD_OLDER))
    }

    /// The status bar's hints for the list's keys in the room.
    pub(super) fn activity_hints(&self) -> Option<hints::ActivityHints> {
        self.in_activity().then(|| {
            let selected = (self.activity.selected).and_then(|seq| self.activity.feed.entry(seq));
            hints::ActivityHints {
                rows: !self.activity.feed.is_empty(),
                selected: selected.is_some(),
                undo: selected.is_some_and(|entry| {
                    let (confirmed, pending) = self.store.activity_flags(entry);
                    let group = self.activity.undos.group_outcome(entry.group);
                    feed::undo_action(entry, confirmed, pending, group) == UndoAction::Undo
                }),
            }
        })
    }

    /// A quiet button that acts at once, a Tab stop with its focus ring: Enter or Space presses it
    /// (GPUI clicks a focused element), Escape goes back to the list.
    fn tool_button(
        &self,
        tool: Tool,
        window: &Window,
        cx: &mut Context<Self>,
        press: fn(&mut Self, &mut Context<Self>),
    ) -> Div {
        let Tool {
            stop,
            icon,
            label,
            busy,
            dot,
        } = tool;
        let p = self.palette;
        let focused = self.stop_handle(stop).is_focused(window);
        let ink = if busy { p.ink_3 } else { p.ink_2 };
        let button = div()
            .id(SharedString::from(format!("{stop:?}")))
            .relative()
            .h(px(component::BUTTON_SMALL))
            .px(px(space::S3))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::S2))
            .border(px(elevation::HAIRLINE))
            .bg(p.raised)
            .text_color(ink)
            .style(ty::BUTTON_SMALL)
            .track_focus(self.stop_handle(stop))
            .when(!busy, |b| b.cursor_pointer().hover(|s| s.bg(p.hover)))
            .child(kit::icon(icon, component::ICON_SMALL, ink))
            .child(label)
            .when(dot, |b| {
                b.child(
                    div()
                        .size(px(component::CHIP_DOT))
                        .rounded_full()
                        .bg(p.accent),
                )
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                press(this, cx);
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                this.control_key(e, window, cx);
            }));
        // The focus ring: a band of the room tint around the button, its outline in the accent.
        div()
            .flex_none()
            .p(px(elevation::FOCUS_RING))
            .m(px(-elevation::FOCUS_RING))
            .rounded(px(radius::BUTTON + elevation::FOCUS_RING))
            .when(focused, |d| d.bg(p.tint))
            .child(button.rounded(px(radius::BUTTON)).border_color(if focused {
                p.accent
            } else {
                p.line_strong
            }))
    }

    /// The toolbar, in the quick-add's place: the order on the left; Clear filter (while filtered)
    /// and Refresh activity on the right. The refresh's dot says there are newer changes than the
    /// feed shows. It fades with the feed as the room changes.
    pub(super) fn activity_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let swap = self.motion.navigation.list(self.frame);
        let filtered = self.activity.agent.is_some();
        let news = self.activity.feed.loaded()
            && (self.store.activity_revision()).is_some_and(|r| Some(r) != self.activity.revision);
        let refreshing = self.activity.feed.refreshing();
        div()
            .h(px(desktop::QUICK_ADD))
            .my(px(desktop::QUICK_ADD_MARGIN))
            .mx(px(desktop::LIST_PAD_X))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(space::S3))
            .opacity(swap.incoming)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(kit::icon(Icon::Clock, component::ICON_SMALL, p.ink_3))
                    .child(kit::label(&p, feed::NEWEST_FIRST)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .when(filtered, |d| {
                        d.child(self.tool_button(
                            Tool {
                                stop: Stop::ActivityFilter,
                                icon: Icon::X,
                                label: feed::CLEAR_FILTER,
                                busy: false,
                                dot: false,
                            },
                            window,
                            cx,
                            |this, cx| this.select_room(RoomKey::Activity(None), cx),
                        ))
                    })
                    .child(self.tool_button(
                        Tool {
                            stop: Stop::ActivityRefresh,
                            icon: Icon::RefreshCw,
                            label: feed::REFRESH,
                            busy: refreshing,
                            dot: news,
                        },
                        window,
                        cx,
                        |this, cx| {
                            if !this.activity.feed.refreshing() {
                                this.refresh_activity(cx);
                            }
                        },
                    )),
            )
    }

    /// The feed: the changes by day, or the empty line, with what's left to read at the end.
    pub(super) fn activity_feed(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let room = &self.shell.room;
        let f = &self.activity.feed;
        if f.is_empty_feed() {
            return self
                .empty_list(&room.empty_line, &room.empty_hint)
                .into_any_element();
        }
        let p = self.palette;
        let days = self.activity_days();
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("activity-feed")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.activity_scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .pb(px(space::S6))
                            .children(days.iter().map(|day| self.activity_day(day, true, cx)))
                            .child(self.activity_end(window, cx)),
                    ),
            )
            .child(
                self.activity_bar
                    .overlay(&self.activity_scroll, &p, self.animate),
            )
            .into_any_element()
    }

    /// The paint-only feed of a room being left (the room switch's outgoing layer), at its scroll.
    pub(super) fn activity_snapshot(
        &self,
        days: &[ActivityDay],
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .relative()
            .top(top)
            .flex()
            .flex_col()
            .children(days.iter().map(|day| self.activity_day(day, false, cx)))
    }

    fn activity_day(&self, day: &ActivityDay, live: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let n = day.rows.len();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pt(px(space::S5))
                    .pb(px(space::S2))
                    .px(px(desktop::LIST_PAD_X))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(p.line)
                    .child(
                        div()
                            .style(ty::CAPS_LABEL)
                            .text_color(p.ink_2)
                            .child(day.label.to_uppercase()),
                    )
                    .child(
                        div()
                            .style(ty::COUNT)
                            .text_color(p.ink_3)
                            .child(format!("{n} {}", if n == 1 { "change" } else { "changes" })),
                    ),
            )
            .children(day.rows.iter().map(|row| self.change_row(row, live, cx)))
    }

    /// Who made the change: the agent's letter mark, a neutral `Y` for the owner, a mail or clock
    /// icon for email and the system.
    fn change_mark(&self, mark: Mark) -> Div {
        let p = self.palette;
        let size = component::AGENT_MARK;
        let circle = |ink: Hsla| {
            div()
                .size(px(size))
                .flex_none()
                .rounded_full()
                .border(px(elevation::HAIRLINE))
                .border_color(p.line_strong)
                .bg(p.raised)
                .flex()
                .items_center()
                .justify_center()
                .text_color(ink)
        };
        match mark {
            Mark::Agent(agent) => kit::agent_mark(self.theme, agent, size),
            Mark::You => circle(p.ink_2)
                .style(ty::AGENT_LETTER)
                .text_size(px(size * ty::AGENT_LETTER.size / component::AGENT_MARK))
                .child("Y"),
            Mark::Mail => {
                circle(p.ink_2).child(kit::icon(Icon::Mail, component::ICON_SMALL, p.ink_2))
            }
            Mark::Other => {
                circle(p.ink_3).child(kit::icon(Icon::Clock, component::ICON_SMALL, p.ink_3))
            }
        }
    }

    fn change_row(&self, row: &ActivityRow, live: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.palette;
        let seq = row.seq;
        let look = if live {
            self.activity.motion.look(seq, row.selected, self.frame)
        } else {
            activity::FeedLook {
                opacity: 1.,
                rise: 0.,
                selected: f32::from(row.selected),
                note: 1.,
            }
        };
        let undone = row.action == UndoAction::Undone;
        let dot = || {
            div()
                .size(px(elevation::SELECTION_BAR))
                .rounded_full()
                .bg(p.ink_4)
        };
        let meta = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .style(ty::META)
            .text_color(p.ink_3)
            .child(div().text_color(p.ink_2).child(row.who.clone()))
            .child(dot())
            .child(row.when.clone())
            .child(dot())
            .child(div().style(ty::COUNT).child(row.clock.clone()));
        let refusal = row.refusal.map(|why| {
            div()
                .pt(px(desktop::ROW_LINE_GAP))
                .flex()
                .items_center()
                .gap(px(space::S1 + space::S1 / 2.))
                .opacity(look.note)
                .style(ty::META)
                .text_color(p.danger_ink)
                .child(kit::icon(
                    Icon::CircleAlert,
                    component::ICON_SMALL,
                    p.danger_ink,
                ))
                .child(why)
        });
        let action = self.change_action(row, live, cx);
        let el = div()
            .id(SharedString::from(format!("change-{seq}")))
            .relative()
            .top(px(look.rise))
            .opacity(look.opacity)
            .w_full()
            .min_h(px(desktop::ROW))
            .flex()
            .flex_none()
            .items_start()
            .gap(px(desktop::ROW_GAP))
            .pt(px(desktop::ROW_PAD_TOP + space::S1 / 2.))
            .pb(px(desktop::ROW_PAD_TOP))
            .px(px(desktop::LIST_PAD_X))
            .border_b_1()
            .border_color(p.line)
            .bg(theme::with_alpha(p.wash, look.selected))
            .child(
                div()
                    .w(px(desktop::COL_CHECK))
                    .pt(px(space::S1 / 2.))
                    .flex_none()
                    .child(self.change_mark(row.mark)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(desktop::ROW_LINE_GAP))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .style(ty::TASK_TITLE)
                            .text_color(if undone { p.ink_3 } else { p.ink })
                            .child(row.what.clone()),
                    )
                    .child(meta)
                    .children(refusal),
            )
            .child(action)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(-elevation::HAIRLINE))
                    .bottom(px(-elevation::HAIRLINE))
                    .w(px(elevation::SELECTION_BAR))
                    .bg(theme::with_alpha(p.accent, look.selected)),
            );
        if !live {
            return el;
        }
        let bounds = self.activity_rows.clone();
        let hover = p.hover;
        el.when(!row.selected, |el| el.hover(move |s| s.bg(hover)))
            .child(
                gpui::canvas(
                    move |b, _, _| {
                        bounds.borrow_mut().insert(seq, b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select_change(Some(seq), cx);
            }))
    }

    /// A change's Undo: a button while it can be undone (Undoing while it waits), else quiet words.
    fn change_action(&self, row: &ActivityRow, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let seq = row.seq;
        let label = row.action.label();
        if !row.action.is_button() {
            let (icon, ink) = match row.action {
                UndoAction::Undone => (Icon::Check, p.ink_3),
                _ => (Icon::Minus, p.ink_4),
            };
            return div()
                .h(px(component::BUTTON_SMALL))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(space::S1))
                .style(ty::META)
                .text_color(ink)
                .child(kit::icon(icon, component::ICON_SMALL, ink))
                .child(label)
                .into_any_element();
        }
        let ready = row.action == UndoAction::Undo;
        let ink = if ready { p.accent_strong } else { p.ink_3 };
        div()
            .id(SharedString::from(format!("undo-{seq}")))
            .h(px(component::BUTTON_SMALL))
            .px(px(space::S3))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::S1 + space::S1 / 2.))
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if ready { p.line_strong } else { p.line })
            .bg(if ready { p.raised } else { p.pane })
            .style(ty::BUTTON_SMALL)
            .text_color(ink)
            .child(kit::icon(Icon::UndoTwo, component::ICON_SMALL, ink))
            .child(label)
            .when(ready && live, |b| {
                b.cursor_pointer()
                    .hover(|s| s.bg(p.wash))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        window.focus(&this.list_focus, cx);
                        this.select_change(Some(seq), cx);
                        this.undo_change(seq, cx);
                    }))
            })
            .into_any_element()
    }

    /// The end of the feed: reading the next page, the button that reads it (or the failed one
    /// again), or why the last read failed.
    fn activity_end(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let f = &self.activity.feed;
        let loading = f.pending().is_some();
        let error = f.error();
        let mut end = div()
            .px(px(desktop::LIST_PAD_X))
            .pt(px(space::S5))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(space::S3));
        if let Some(error) = error {
            end = end.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .style(ty::BODY)
                    .text_color(p.danger_ink)
                    .child(kit::icon(
                        Icon::CircleAlert,
                        component::ICON_SMALL,
                        p.danger_ink,
                    ))
                    .child(error.to_string()),
            );
        }
        if loading {
            end = end.child(
                div()
                    .style(ty::META)
                    .text_color(p.ink_3)
                    .child(feed::LOADING),
            );
        } else if let Some((_, label)) = self.more_button() {
            let icon = if error.is_some() {
                Icon::RefreshCw
            } else {
                Icon::ChevronDown
            };
            end = end.child(self.tool_button(
                Tool {
                    stop: Stop::ActivityMore,
                    icon,
                    label,
                    busy: false,
                    dot: false,
                },
                window,
                cx,
                |this, cx| {
                    if let Some((before, _)) = this.more_button() {
                        this.load_activity(before, cx);
                    }
                },
            ));
        }
        end
    }
}
