//! The screen shown when the paired data couldn't be opened: what happened and **Try again**. When a
//! recovery of damaged data couldn't finish, it also offers to start fresh from the server, keeping the
//! damaged copy, after asking: that gives up on the changes that hadn't been sent. A start fresh that was
//! interrupted can only be finished.

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Window, div, prelude::*, px,
};

use super::kit;
use crate::model::Smart;
use crate::session::Pending;
use crate::theme::{self, TextStyled, Theme, elevation, radius, space, ty};

/// What the owner chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedEvent {
    /// Open the saved pairing again.
    Retry,
    /// Confirmed (or already under way): set the damaged data aside and start with an empty cache.
    StartFresh,
}

/// The label of the button that offers to start fresh.
pub const START_FRESH: &str = "Start fresh (keep the damaged copy)";

/// The paired data couldn't be opened.
#[allow(missing_debug_implementations)] // Holds a GPUI focus handle.
pub struct FailedView {
    theme: Theme,
    why: SharedString,
    /// What opening stopped at: a recovery (starting fresh is offered too) or a start fresh (only
    /// finishing it is offered).
    pending: Option<Pending>,
    /// Start fresh was chosen and is being confirmed.
    confirming: bool,
    focus: FocusHandle,
}

impl EventEmitter<FailedEvent> for FailedView {}

impl FailedView {
    pub fn new(
        theme: Theme,
        why: SharedString,
        pending: Option<Pending>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            why,
            pending,
            confirming: false,
            focus: cx.focus_handle(),
        }
    }

    /// The screenshot fixture: a recovery that can't read the damaged copy, before or while confirming.
    pub fn fixture(theme: Theme, confirming: bool, cx: &mut Context<Self>) -> Self {
        let why = "The local data can't be opened: the unsent changes couldn't be read back from \
                   cache.damaged-1763721300000.sqlite yet: file is not a database.";
        Self {
            confirming,
            ..Self::new(theme, why.into(), Some(Pending::Recovery), cx)
        }
    }

    fn ask(&mut self, confirming: bool, cx: &mut Context<Self>) {
        self.confirming = confirming;
        cx.notify();
    }
}

impl Focusable for FailedView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FailedView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(self.theme, Smart::MyDay.hue());
        let recovery = self.pending == Some(Pending::Recovery);
        let finishing = self.pending == Some(Pending::StartFresh);
        let hint = match self.pending {
            Some(Pending::Recovery) => {
                "The local data was damaged, and the changes that hadn't reached the server yet \
                 couldn't be read back from it. Nothing has been removed. Try again, or start fresh \
                 from the server."
            }
            Some(Pending::StartFresh) => {
                "Starting fresh was interrupted while the damaged data was being set aside. Finish it \
                 to open your tasks; the damaged copy is kept."
            }
            None => "Nothing was changed or removed. Close other copies of the app and try again.",
        };
        let actions = div()
            .flex()
            .gap(px(space::S3))
            .when(finishing, |d| {
                d.child(
                    kit::button(&p, "Finish starting fresh")
                        .id("finish-start-fresh")
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(FailedEvent::StartFresh))),
                )
            })
            .when(!finishing, |d| {
                d.child(
                    kit::button(&p, "Try again")
                        .id("retry")
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(FailedEvent::Retry))),
                )
            })
            .when(recovery && !self.confirming, |d| {
                d.child(
                    kit::button_quiet(&p, START_FRESH)
                        .id("start-fresh")
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.ask(true, cx))),
                )
            });
        let confirm = (recovery && self.confirming).then(|| {
            div()
                .id("start-fresh-confirm")
                .mt(px(space::S2))
                .p(px(space::S4))
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .rounded(px(radius::LG))
                .border(px(elevation::HAIRLINE))
                .border_color(p.line_strong)
                .bg(p.raised)
                .child(div().style(ty::TASK_TITLE).child("Start fresh?"))
                .child(div().text_color(p.ink_2).child(
                    "Changes made on this computer that hadn't reached the server won't be \
                     recovered. Your tasks are loaded again from the server. The damaged copy is \
                     kept in the app's data folder until you sign out.",
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S3))
                        .child(
                            kit::button(&p, "Start fresh")
                                .id("start-fresh-yes")
                                .bg(p.danger)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.ask(false, cx);
                                    cx.emit(FailedEvent::StartFresh);
                                })),
                        )
                        .child(
                            kit::button_quiet(&p, "Cancel")
                                .id("start-fresh-no")
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.ask(false, cx))),
                        ),
                )
        });
        div()
            .id("failed")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(p.pane)
            .style(ty::BODY)
            .text_color(p.ink)
            .child(
                div()
                    .h(px(theme::desktop::TITLEBAR))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .child(kit::window_controls(&p)),
            )
            .child(
                div()
                    .w(px(560.))
                    .mx_auto()
                    .pt(px(space::S10 * 2.))
                    .flex()
                    .flex_col()
                    .gap(px(space::S4))
                    .child(
                        div()
                            .style(ty::ROOM_TITLE)
                            .child("Your tasks couldn't be opened"),
                    )
                    .child(div().text_color(p.ink_2).child(self.why.clone()))
                    .child(div().text_color(p.ink_3).child(hint))
                    .child(actions)
                    .children(confirm),
            )
    }
}
