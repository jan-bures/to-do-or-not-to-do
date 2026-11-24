//! The quick-add window that Ctrl+Alt+Space opens from anywhere.
//!
//! It is created hidden at startup and only shown and hidden afterwards, so the hotkey never waits for a
//! window to be created. A slim sky with the day's sun sits over the input, so it reads as the same room.

use std::rc::Rc;

use todo_client::intent::{self, TaskRoom, quick_add::Draft};
use todo_core::{ids::ListId, model::List};

use gpui::{
    App, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Pixels,
    Role, SharedString, Size, Window, WindowBounds, WindowKind, WindowOptions, div, point,
    prelude::*, px,
};

use super::input::{InputEvent, TextInput};
use super::kit;
use super::landscape::{LandscapeCache, Sky, landscape, sun_at};
use super::shell::quick_add_colors;
use crate::model::{Smart, lists};
use crate::platform::win;
use crate::store::Store;
use crate::theme::{
    self, Icon, Palette, TextStyled, Theme, component, desktop, elevation, radius, space, ty,
};

/// The window's logical size.
pub const SIZE: Size<Pixels> = Size {
    width: px(desktop::QUICK_ADD_POPUP_WIDTH),
    height: px(desktop::QUICK_ADD_POPUP_HEIGHT),
};

/// What the quick-add window reports to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickAddEvent {
    Submitted(Draft),
    Dismissed,
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct QuickAddView {
    palette: Palette,
    theme: Theme,
    room_title: SharedString,
    done: u32,
    total: u32,
    input: Entity<TextInput>,
    store: Option<Rc<Store>>,
    draft: Draft,
    list_dismissed: bool,
    destination: DestinationControls,
    landscape: LandscapeCache,
    /// Whether Ctrl+Alt+Space is registered; the hint is dropped when another app owns it.
    hotkey: bool,
}

impl EventEmitter<QuickAddEvent> for QuickAddView {}

impl QuickAddView {
    /// A quick-add into My Day, whose progress (`done` of `total`) the sun shows.
    pub fn new(
        (done, total): (u32, u32),
        theme: Theme,
        typed: Option<&str>,
        hotkey: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = theme::palette(theme, Smart::MyDay.hue());
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
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => {
                    let draft = std::mem::take(&mut this.draft);
                    this.list_dismissed = false;
                    input.update(cx, |i, cx| i.reset(window, cx));
                    if draft.text.trim().is_empty() {
                        cx.emit(QuickAddEvent::Dismissed);
                    } else {
                        cx.emit(QuickAddEvent::Submitted(draft));
                    }
                }
                InputEvent::Cancel => cx.emit(QuickAddEvent::Dismissed),
                InputEvent::Changed => {
                    let text = input.read(cx).content.to_string();
                    if text.trim().is_empty() {
                        this.draft.list = None;
                        this.list_dismissed = false;
                    }
                    this.draft.text = text;
                    cx.notify();
                }
            },
        )
        .detach();
        Self {
            palette,
            theme,
            room_title: Smart::MyDay.label().into(),
            done,
            total,
            input,
            store: None,
            draft: Draft {
                text: typed.unwrap_or_default().into(),
                ..Draft::default()
            },
            list_dismissed: false,
            destination: DestinationControls::new(cx),
            landscape: LandscapeCache::default(),
            hotkey,
        }
    }

    pub fn input(&self) -> &Entity<TextInput> {
        &self.input
    }

    /// Updates the suggested destination and the sun when the replica changes.
    pub fn set_room(
        &mut self,
        (done, total): (u32, u32),
        store: Option<Rc<Store>>,
        cx: &mut Context<Self>,
    ) {
        if !matches!((&self.store, &store), (Some(a), Some(b)) if Rc::ptr_eq(a, b)) {
            self.draft.list = None;
            self.list_dismissed = false;
        }
        self.store = store;
        if self.draft.list.is_some()
            && self.store.as_ref().is_none_or(|store| {
                intent::chosen_destination(&store.view(), TaskRoom::MyDay, &self.draft).is_none()
            })
        {
            self.draft.list = None;
            self.list_dismissed = true;
        }
        self.done = done;
        self.total = total;
        cx.notify();
    }

    fn quick_destination(&self) -> Option<List> {
        self.store
            .as_ref()?
            .quick_destination(TaskRoom::MyDay, &self.draft, self.list_dismissed)
    }

    fn choose_destination(
        &mut self,
        choice: Option<ListId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.draft.list = choice;
        self.list_dismissed = choice.is_none();
        window.focus(&self.focus_handle(cx), cx);
        cx.notify();
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
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .children(self.quick_destination().map(|list| {
                self.destination.render(
                    &list,
                    self.draft.list.is_some(),
                    (self.theme, p),
                    window,
                    cx,
                    Self::choose_destination,
                )
            }))
            .child(kit::kbd(&p, "Enter"));
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(component::META_GAP))
                .child(kit::kbd(&p, key))
                .child(what)
        };
        div()
            .id("quick-add")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "tab" || this.quick_destination().is_none() {
                    return;
                }
                let input = this.focus_handle(cx);
                let mut order = vec![&input];
                if this.draft.list.is_none() {
                    order.push(&this.destination.accept);
                }
                order.push(&this.destination.remove);
                let current = order
                    .iter()
                    .position(|handle| handle.is_focused(window))
                    .unwrap_or(0);
                let step = if event.keystroke.modifiers.shift {
                    order.len() - 1
                } else {
                    1
                };
                window.focus(order[(current + step) % order.len()], cx);
                cx.stop_propagation();
            }))
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
                                            .text_color(p.accent_strong)
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
                        sun_at(self.done, self.total),
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

pub(super) struct DestinationControls {
    pub accept: FocusHandle,
    pub remove: FocusHandle,
}

impl DestinationControls {
    pub fn new<T: 'static>(cx: &Context<T>) -> Self {
        Self {
            accept: cx.focus_handle(),
            remove: cx.focus_handle(),
        }
    }

    pub fn render<T: 'static>(
        &self,
        list: &List,
        accepted: bool,
        (theme, p): (Theme, Palette),
        window: &Window,
        cx: &Context<T>,
        choose: fn(&mut T, Option<ListId>, &mut Window, &mut Context<T>),
    ) -> Div {
        let hue = lists::hue(list.color.as_deref(), list.id.as_uuid().as_bytes());
        let swatch = theme::list_colors(theme, hue).swatch;
        let name = if accepted {
            list.name.clone()
        } else {
            format!("In {}?", list.name)
        };
        let remove = if accepted {
            format!("Remove destination: {}", list.name)
        } else {
            format!("Dismiss list suggestion: {}", list.name)
        };
        let action = |id, label: String, focus: &FocusHandle, choice, content: Div| {
            content
                .id(id)
                .track_focus(focus)
                .role(Role::Button)
                .aria_label(label.clone())
                .tooltip(kit::tooltip(p, label))
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .rounded(px(radius::MD))
                .border(px(elevation::HAIRLINE))
                .border_color(if focus.is_focused(window) {
                    p.accent
                } else {
                    gpui::transparent_black()
                })
                .on_click(cx.listener(move |this, _, window, cx| choose(this, choice, window, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        choose(this, choice, window, cx);
                        cx.stop_propagation();
                    }
                }))
        };
        let label = div()
            .flex()
            .items_center()
            .min_w_0()
            .max_w(px(desktop::CHIP_MAX))
            .gap(px(space::S1))
            .h_full()
            .px(px(space::S2))
            .child(
                div()
                    .flex_none()
                    .size(px(space::S2))
                    .rounded_full()
                    .bg(swatch),
            )
            .child(div().truncate().child(name.clone()));
        let dismiss = div()
            .flex_none()
            .flex()
            .items_center()
            .h_full()
            .px(px(space::S1))
            .child(kit::icon(Icon::X, component::ICON_SMALL, p.ink));
        div()
            .flex()
            .items_center()
            .min_w_0()
            .h(px(space::S6))
            .rounded(px(radius::MD))
            .border(px(elevation::HAIRLINE))
            .border_color(if accepted { p.wash } else { p.line })
            .bg(if accepted { p.wash } else { p.raised })
            .text_color(p.ink)
            .style(ty::COUNT.with_weight(ty::DUE.weight))
            .child(if accepted {
                label.into_any_element()
            } else {
                action(
                    "quick-accept-list",
                    name,
                    &self.accept,
                    Some(list.id),
                    label,
                )
                .into_any_element()
            })
            .child(action(
                "quick-remove-list",
                remove,
                &self.remove,
                None,
                dismiss,
            ))
    }
}

/// Create the hidden popup at its real size on the primary display. It is repositioned on show.
pub fn window_options(cx: &App) -> WindowOptions {
    let bounds = match cx.primary_display() {
        Some(display) => {
            let display = display.bounds();
            Bounds::new(
                point(
                    display.center().x - SIZE.width / 2.,
                    display.origin.y + display.size.height * desktop::QUICK_ADD_POPUP_TOP_FRACTION,
                ),
                SIZE,
            )
        }
        None => Bounds::centered(None, SIZE, cx),
    };
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("to-do-or-not-to-do".into()),
        ..WindowOptions::default()
    }
}

pub fn position(hwnd: isize, target: win::PopupTarget) -> anyhow::Result<()> {
    win::position_popup(
        hwnd,
        win::popup_rect(target, SIZE, desktop::QUICK_ADD_POPUP_TOP_FRACTION),
    )
}

/// Re-anchor before showing, so there's no frame on the wrong monitor or at the wrong DPI.
pub fn show(hwnd: isize) {
    if hwnd != 0 {
        if let Err(error) = win::cursor_popup_target().and_then(|target| position(hwnd, target)) {
            tracing::warn!(%error, "couldn't position quick add");
        }
        win::show_and_focus(hwnd);
    }
}
