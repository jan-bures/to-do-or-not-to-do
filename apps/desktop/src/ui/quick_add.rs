//! The quick-add window that Ctrl+Alt+Space opens from anywhere.
//!
//! It is created hidden at startup and only shown and hidden afterwards, so the hotkey never waits for a
//! window to be created. A slim sky with the day's sun sits over the input, so it reads as the same room.

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Size, Window,
    WindowBounds, WindowKind, WindowOptions, div, point, prelude::*, px, size,
};

use super::input::{InputEvent, TextInput};
use super::kit;
use super::landscape::{LandscapeCache, Sky, landscape};
use super::shell::quick_add_colors;
use crate::fixtures::Room;
use crate::theme::{
    self, Icon, Palette, TextStyled, Theme, component, desktop, elevation, radius, space, ty,
};

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
    /// Whether Ctrl+Alt+Space is registered; the hint is dropped when another app owns it.
    hotkey: bool,
}

impl EventEmitter<QuickAddEvent> for QuickAddView {}

impl QuickAddView {
    pub fn new(
        room: &Room,
        theme: Theme,
        typed: Option<&str>,
        hotkey: bool,
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
            hotkey,
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
        let bar = || {
            div()
                .w(px(elevation::HAIRLINE))
                .h(px(ty::CAPS_LABEL.size - 0.5))
                .bg(p.line_strong)
        };
        let summary = if self.total == 0 {
            "nothing planned yet".to_string()
        } else {
            format!("{} of {} done", self.done, self.total)
        };
        let field = div()
            .h(px(desktop::QUICK_ADD))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S3 + 2.))
            .pr(px(space::S2 + 2.))
            .rounded(px(radius::QUICK_ADD))
            .border(px(elevation::HAIRLINE))
            .border_color(if active { p.accent } else { p.control })
            .bg(p.raised)
            .style(ty::QUICK_ADD)
            .child(kit::icon(Icon::Plus, component::ICON, p.accent_strong))
            .child(self.input.clone())
            .child(kit::kbd(&p, "Enter"));
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(component::CHIP_GAP))
                .child(kit::kbd(&p, key))
                .child(what)
        };
        div()
            .id("quick-add")
            .size_full()
            .flex()
            .flex_col()
            .bg(p.pane)
            .border(px(elevation::HAIRLINE))
            .border_color(p.line_strong)
            .style(ty::BODY)
            .text_color(p.ink)
            .child(
                div()
                    .flex_none()
                    .bg(p.sky)
                    .child(
                        div()
                            .pt(px(space::S3 + 2.))
                            .px(px(space::S5))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2 + 2.))
                                    .h(px(ty::EYEBROW_DAY.line_height))
                                    .child(
                                        div()
                                            .style(ty::EYEBROW_DAY)
                                            .text_color(p.ink_2)
                                            .child("Quick add"),
                                    )
                                    .child(bar())
                                    .child(
                                        div().style(ty::EYEBROW_META).text_color(p.ink_3).child(
                                            format!("to {}", self.room_title).to_uppercase(),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .style(ty::SUMMARY_FIGURES)
                                    .text_color(p.ink_2)
                                    .child(summary),
                            ),
                    )
                    .child(landscape(
                        &self.landscape,
                        Sky::Slim(p),
                        self.done,
                        self.total,
                    )),
            )
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
            .child(
                div()
                    .px(px(space::S4 - elevation::FOCUS_RING))
                    .pt(px(space::S4 - elevation::FOCUS_RING))
                    .child(
                        div()
                            .p(px(elevation::FOCUS_RING))
                            .rounded(px(radius::QUICK_ADD + elevation::FOCUS_RING))
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
                    .px(px(space::S5))
                    .pt(px(space::S2))
                    .flex()
                    .gap(px(space::S4 + 2.))
                    .style(ty::META)
                    .text_color(p.ink_3)
                    .child(hint("Enter", "add"))
                    .child(hint("Esc", "close"))
                    .when(self.hotkey, |row| {
                        row.child(hint("Ctrl Alt Space", "from anywhere"))
                    }),
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
