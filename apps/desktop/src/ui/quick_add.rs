//! The quick-add window that Ctrl+Alt+Space opens from anywhere.
//!
//! It is created hidden at startup and only shown and hidden afterwards, so the hotkey never waits for a
//! window to be created. A slim sky with the day's sun sits over the input, so it reads as the same room.

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Size, Window,
    WindowBounds, WindowKind, WindowOptions, div, point, prelude::*, px, size,
};

use super::input::{InputEvent, TextInput};
use super::kit::{self, Type};
use super::landscape::{LandscapeCache, landscape};
use super::shell::quick_add_colors;
use crate::fixtures::Room;
use crate::theme::{self, Palette, Theme, radius, space, text, weight};

/// The window's logical size.
pub const SIZE: Size<Pixels> = Size {
    width: px(640.),
    height: px(196.),
};

/// What the quick-add window reports to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickAddEvent {
    Submitted(String),
    Dismissed,
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct QuickAddView {
    palette: Palette,
    room_title: &'static str,
    done: u32,
    total: u32,
    input: Entity<TextInput>,
    landscape: LandscapeCache,
}

impl EventEmitter<QuickAddEvent> for QuickAddView {}

impl QuickAddView {
    pub fn new(
        room: &Room,
        theme: Theme,
        typed: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = theme::palette(theme, room.hue);
        let input = cx.new(|cx| {
            let mut input = TextInput::new("Add a task to My Day…", quick_add_colors(&palette), cx);
            if let Some(t) = typed {
                input.set_text(t.to_string(), cx);
            }
            input
        });
        cx.subscribe_in(
            &input,
            window,
            |_this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => {
                    let title = input.read(cx).content.trim().to_string();
                    input.update(cx, |i, cx| i.reset(window, cx));
                    if title.is_empty() {
                        cx.emit(QuickAddEvent::Dismissed);
                    } else {
                        cx.emit(QuickAddEvent::Submitted(title));
                    }
                }
                InputEvent::Cancel => cx.emit(QuickAddEvent::Dismissed),
                InputEvent::Changed => cx.notify(),
            },
        )
        .detach();
        Self {
            palette,
            room_title: room.title,
            done: room.done,
            total: room.total,
            input,
            landscape: LandscapeCache::default(),
        }
    }

    pub fn input(&self) -> &Entity<TextInput> {
        &self.input
    }
}

impl Focusable for QuickAddView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle.clone()
    }
}

impl Render for QuickAddView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let focused = self.input.read(cx).focus_handle.is_focused(window);
        let typed = !self.input.read(cx).content.is_empty();
        let active = focused || typed;
        let bar = || div().w(px(1.)).h(px(10.)).bg(p.line_strong);
        let summary = if self.total == 0 {
            "nothing planned yet".to_string()
        } else {
            format!("{} of {} done", self.done, self.total)
        };
        let field = div()
            .h(px(space::QUICK_ADD_H))
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(14.))
            .pr(px(10.))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if active { p.accent } else { p.control })
            .bg(p.raised)
            .ui(text::QUICK_ADD, weight::REGULAR)
            .child(kit::icon("plus", 18., p.accent_strong))
            .child(self.input.clone())
            .child(kit::kbd(&p, "Enter"));
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(kit::kbd(&p, key))
                .child(what)
        };
        div()
            .id("quick-add")
            .size_full()
            .flex()
            .flex_col()
            .bg(p.pane)
            .border_1()
            .border_color(p.line_strong)
            .font_family(theme::font::UI)
            .text_color(p.ink)
            .child(
                div()
                    .flex_none()
                    .bg(p.sky)
                    .child(
                        div()
                            .pt(px(14.))
                            .px(px(20.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .h(px(22.))
                                    .child(
                                        div()
                                            .display(text::EYEBROW_DOW, weight::REGULAR)
                                            .italic()
                                            .text_color(p.ink_2)
                                            .child("Quick add"),
                                    )
                                    .child(bar())
                                    .child(
                                        div()
                                            .mono(text::EYEBROW_MONO, weight::REGULAR)
                                            .text_color(p.ink_3)
                                            .child(
                                                format!("to {}", self.room_title).to_uppercase(),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .mono((12., 16.), weight::REGULAR)
                                    .text_color(p.ink_2)
                                    .child(summary),
                            ),
                    )
                    .child(landscape(
                        &self.landscape,
                        p,
                        self.done,
                        self.total,
                        self.total == 0,
                    )),
            )
            .child(div().h(px(1.)).bg(p.line))
            .child(
                div().px(px(13.)).pt(px(13.)).child(
                    div()
                        .p(px(3.))
                        .rounded(px(radius::LG + 3.))
                        .bg(if active {
                            p.tint
                        } else {
                            gpui::transparent_black()
                        })
                        .child(field),
                ),
            )
            .child(
                div()
                    .px(px(20.))
                    .pt(px(8.))
                    .flex()
                    .gap(px(18.))
                    .ui((12., 16.), weight::REGULAR)
                    .text_color(p.ink_3)
                    .child(hint("Enter", "add"))
                    .child(hint("Esc", "close"))
                    .child(hint("Ctrl Alt Space", "from anywhere")),
            )
    }
}

/// Where the quick-add window opens: horizontally centred, a fifth of the way down the display.
pub fn window_options(cx: &App, show: bool) -> WindowOptions {
    let centered = Bounds::centered(None, SIZE, cx);
    let display_top = cx
        .primary_display()
        .map(|d| d.bounds().origin.y)
        .unwrap_or_default();
    let display_h = cx
        .primary_display()
        .map(|d| d.bounds().size.height)
        .unwrap_or(px(900.));
    let origin = point(centered.origin.x, display_top + display_h * 0.2);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            origin,
            size(SIZE.width, SIZE.height),
        ))),
        titlebar: None,
        focus: show,
        show,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("to-do-or-not-to-do".into()),
        ..WindowOptions::default()
    }
}
