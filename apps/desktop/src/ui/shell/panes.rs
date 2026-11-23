//! Splitter hit targets, capture, and short-lived visual feedback. The model owns sizing and limits.

use gpui::{
    CursorStyle, DispatchPhase, KeyDownEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Orientation, Role, canvas,
};

use super::*;
use crate::config::UiPreferences;
use crate::model::layout::{Columns, Grab, KeyResize, Pane};
use crate::theme::motion as m;

struct Grip {
    focus: FocusHandle,
    hovered: bool,
    highlight: Track,
    ring: Track,
    reset: Option<Track>,
    key_resize: KeyResize,
}

impl Grip {
    fn new(cx: &mut Context<ShellView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            hovered: false,
            highlight: Track::rest(0.),
            ring: Track::rest(0.),
            reset: None,
            key_resize: KeyResize::default(),
        }
    }
}

#[derive(Clone, Copy)]
struct Drag {
    grab: Grab,
    x: f32,
    interrupted_reset: bool,
}

pub(super) struct Splitters {
    sidebar: Grip,
    detail: Grip,
    ai: Grip,
    drag: Option<Drag>,
    viewport: f32,
    preview_hover: bool,
}

impl Splitters {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ShellView>) -> Self {
        let this = Self {
            sidebar: Grip::new(cx),
            detail: Grip::new(cx),
            ai: Grip::new(cx),
            drag: None,
            viewport: f32::from(window.viewport_size().width),
            preview_hover: false,
        };
        for pane in [Pane::Sidebar, Pane::Detail, Pane::Ai] {
            let handle = this.focus(pane);
            cx.on_focus(handle, window, |_, _, cx| cx.notify()).detach();
            cx.on_blur(handle, window, move |this, _, cx| {
                this.finish_key_resize(pane);
                cx.notify();
            })
            .detach();
        }
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.finish_key_resize(Pane::Sidebar);
                this.finish_key_resize(Pane::Detail);
                this.finish_key_resize(Pane::Ai);
                this.finish_pane_drag(cx);
            }
        })
        .detach();
        this
    }

    pub(super) fn has_focus(&self, window: &Window) -> bool {
        self.sidebar.focus.is_focused(window)
            || self.detail.focus.is_focused(window)
            || self.ai.focus.is_focused(window)
    }

    pub(super) fn focus(&self, pane: Pane) -> &FocusHandle {
        match pane {
            Pane::Sidebar => &self.sidebar.focus,
            Pane::Detail => &self.detail.focus,
            Pane::Ai => &self.ai.focus,
        }
    }

    fn grip_mut(&mut self, pane: Pane) -> &mut Grip {
        match pane {
            Pane::Sidebar => &mut self.sidebar,
            Pane::Detail => &mut self.detail,
            Pane::Ai => &mut self.ai,
        }
    }
}

fn fade_ms(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

impl ShellView {
    pub fn preview_splitter_hover(&mut self, hover: bool) {
        self.splitters.preview_hover = hover;
    }

    pub(super) fn save_ui_preferences(&mut self) {
        if let Some(session) = self.store.session()
            && let Err(e) = session.data().save_ui(&UiPreferences {
                date_format: self.store.now().date_format,
                collapsed_collections: self.ui.collapsed_collections.clone(),
                board_done: self.ui.board_done.clone(),
                timeline: self.ui.timeline,
                panes: self.ui.panes,
            })
        {
            tracing::warn!("couldn't save window preferences: {e}");
            self.notice = Some(Notice::error("Couldn't save window preferences."));
        }
    }

    pub(super) fn columns(&self, window: &Window) -> Columns {
        let mut prefs = self.ui.panes;
        for (pane, grip) in [
            (Pane::Sidebar, &self.splitters.sidebar),
            (Pane::Detail, &self.splitters.detail),
            (Pane::Ai, &self.splitters.ai),
        ] {
            if let Some(reset) = grip.reset {
                prefs = prefs.with(pane, reset.at(self.frame));
            }
        }
        if let Some(drag) = self.splitters.drag {
            prefs = prefs.with(drag.grab.pane, drag.grab.width_at(drag.x));
        }
        let detail = if self.docked() {
            DetailPane::Docked
        } else {
            DetailPane::Hidden // For width purposes, an overlay is the same as hidden.
        };
        prefs.with_assistant(
            f32::from(window.viewport_size().width),
            detail,
            self.ai_docked(),
        )
    }

    pub(super) fn show_detail_splitter(&self) -> bool {
        self.docked() && self.detail_placement() == DetailPane::Docked
    }

    pub(super) fn drive_splitters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = f32::from(window.viewport_size().width);
        let show_detail = self.show_detail_splitter();
        if self.splitters.viewport != viewport {
            self.finish_pane_drag(cx);
            self.splitters.viewport = viewport;
            self.splitters.sidebar.reset = None;
            self.splitters.detail.reset = None;
            self.splitters.ai.reset = None;
        }
        if !self.ai_docked() {
            if self
                .splitters
                .drag
                .is_some_and(|drag| drag.grab.pane == Pane::Ai)
            {
                self.finish_pane_drag(cx);
            }
            self.splitters.ai.reset = None;
            self.splitters.ai.hovered = false;
        }
        if !show_detail {
            if self
                .splitters
                .drag
                .is_some_and(|drag| drag.grab.pane == Pane::Detail)
            {
                self.finish_pane_drag(cx);
            }
            if self.splitters.detail.focus.is_focused(window) {
                self.finish_key_resize(Pane::Detail);
                window.focus(&self.list_focus, cx);
            }
            let detail = &mut self.splitters.detail;
            detail.hovered = false;
            detail.highlight = Track::rest(0.);
            detail.ring = Track::rest(0.);
            detail.reset = None;
        }
        if !self.animate {
            return;
        }
        let mut moving = false;
        let dragged = self.splitters.drag.map(|drag| drag.grab.pane);
        for (pane, grip) in [
            (Pane::Sidebar, &mut self.splitters.sidebar),
            (Pane::Detail, &mut self.splitters.detail),
            (Pane::Ai, &mut self.splitters.ai),
        ] {
            grip.highlight.aim(
                f32::from(grip.hovered || dragged == Some(pane)),
                self.frame,
                fade_ms(m::HOVER),
                m::STANDARD,
            );
            let focused = grip.focus.is_focused(window) && window.last_input_was_keyboard();
            grip.ring.aim(
                f32::from(focused),
                self.frame,
                fade_ms(m::HOVER),
                if focused { m::STANDARD } else { m::EXIT },
            );
            if grip.reset.is_some_and(|t| !t.moving(self.frame)) {
                grip.reset = None;
            }
            moving |= grip.reset.is_some()
                || grip.highlight.moving(self.frame)
                || grip.ring.moving(self.frame);
        }
        if moving {
            window.request_animation_frame();
        }
    }

    fn reset_pane(&mut self, pane: Pane, window: &Window, cx: &mut Context<Self>) {
        self.finish_key_resize(pane);
        let before = self.ui.panes;
        let (prefs, transition) =
            before.reset(pane, self.columns(window), self.now(), self.pace().reduced);
        self.ui.panes = prefs;
        self.splitters.grip_mut(pane).reset = Some(transition);
        if prefs != before {
            self.save_ui_preferences();
        }
        cx.notify();
    }

    fn pane_down(
        &mut self,
        pane: Pane,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_key_resize(pane);
        self.finish_pane_drag(cx);
        window.focus(self.splitters.focus(pane), cx);
        if e.click_count == 2 {
            self.reset_pane(pane, window, cx);
        } else {
            let x = f32::from(e.position.x);
            let grab = Grab::new(pane, x, self.columns(window));
            let interrupted_reset = self.splitters.grip_mut(pane).reset.take().is_some();
            self.splitters.drag = Some(Drag {
                grab,
                x,
                interrupted_reset,
            });
            cx.notify();
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn finish_pane_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.splitters.drag.take() {
            let width = drag
                .grab
                .moved(drag.x)
                .or_else(|| drag.interrupted_reset.then(|| drag.grab.width_at(drag.x)));
            if let Some(width) = width {
                let prefs = self.ui.panes.with(drag.grab.pane, width);
                if prefs != self.ui.panes {
                    self.ui.panes = prefs;
                    self.save_ui_preferences();
                }
            }
            cx.notify();
        }
    }

    fn finish_key_resize(&mut self, pane: Pane) {
        if self
            .splitters
            .grip_mut(pane)
            .key_resize
            .finish(self.ui.panes)
        {
            self.save_ui_preferences();
        }
    }

    fn pane_key(
        &mut self,
        pane: Pane,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mods = e.keystroke.modifiers;
        if mods.alt || mods.control || mods.platform || mods.function {
            return;
        }
        match e.keystroke.key.as_str() {
            "left" | "right" => {
                self.finish_pane_drag(cx);
                let columns = self.columns(window);
                let grip = self.splitters.grip_mut(pane);
                if let Some(prefs) = grip.key_resize.step(
                    self.ui.panes,
                    columns,
                    pane,
                    e.keystroke.key == "right",
                    mods.shift,
                ) {
                    grip.reset = None;
                    self.ui.panes = prefs;
                    cx.notify();
                }
            }
            "enter" | "home" if !mods.shift => {
                if !e.is_held {
                    self.finish_pane_drag(cx);
                    self.reset_pane(pane, window, cx);
                }
            }
            "escape" if !mods.shift => {
                if !e.is_held {
                    self.finish_key_resize(pane);
                    self.finish_pane_drag(cx);
                    window.focus(&self.list_focus, cx);
                    if self.width == Width::Narrow {
                        self.select_task(None, cx);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(super) fn pane_handle(
        &self,
        pane: Pane,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let columns = self.columns(window);
        let width = columns.width(pane);
        let p = &self.palette;
        let line = elevation::HAIRLINE;
        let bar = desktop::PANE_HANDLE_HIGHLIGHT;
        let line_x = line;
        let (id, label, grip) = match pane {
            Pane::Sidebar => ("sidebar-splitter", "Sidebar width", &self.splitters.sidebar),
            Pane::Detail => (
                "detail-splitter",
                "Task detail width",
                &self.splitters.detail,
            ),
            Pane::Ai => (
                "assistant-splitter",
                "Resize AI assistant",
                &self.splitters.ai,
            ),
        };
        let highlight = if pane == Pane::Sidebar && self.splitters.preview_hover {
            1.
        } else {
            grip.highlight.at(self.frame)
        };
        let ring = grip.ring.at(self.frame);
        div()
            .id(id)
            .role(Role::Splitter)
            .aria_label(label)
            .aria_orientation(Orientation::Vertical)
            .aria_numeric_value(f64::from(width))
            .aria_min_numeric_value(f64::from(pane.min()))
            .aria_max_numeric_value(f64::from(columns.max(pane)))
            .aria_numeric_value_step(f64::from(desktop::PANE_KEY_STEP))
            .aria_description("Use Left and Right arrows to resize, Shift for larger steps, Home or Enter to reset, and Escape to return to the list.")
            .track_focus(&grip.focus)
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(desktop::PANE_HANDLE_HIT))
            .when(pane == Pane::Sidebar, |d| d.left(px(columns.handle_inset(pane))))
            .when(pane != Pane::Sidebar, |d| d.right(px(columns.handle_inset(pane))))
            .cursor(CursorStyle::ResizeLeftRight)
            .block_mouse_except_scroll()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.splitters.grip_mut(pane).hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, window, cx| this.pane_down(pane, e, window, cx)))
            .on_key_down(cx.listener(move |this, e, window, cx| this.pane_key(pane, e, window, cx)))
            .on_key_up(cx.listener(move |this, e: &gpui::KeyUpEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "left" | "right") {
                    this.finish_key_resize(pane);
                    cx.notify();
                }
            }))
            .when(ring > 0., |d| {
                d.child(div().absolute().top_0().bottom_0()
                    .left(px(line_x - elevation::FOCUS_RING))
                    .w(px(line + 2. * elevation::FOCUS_RING))
                    .bg(theme::with_alpha(p.tint, p.tint.a * ring)))
                .child(div().absolute().top_0().bottom_0().left(px(line_x)).w(px(line))
                    .bg(theme::with_alpha(p.accent, ring)))
            })
            .when(highlight > 0., |d| {
                d.child(div().absolute().top_0().bottom_0().left(px(line_x + (line - bar) / 2.))
                    .w(px(bar)).bg(theme::with_alpha(p.accent, highlight)))
            })
    }

    /// Windows gives GPUI native mouse capture. Install window-level listeners even before pressing,
    /// so a quick release between paints or outside the window also ends the gesture. Movement uses
    /// pointer events and requests a new paint only when the visible width changes.
    pub(super) fn pane_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let moved = view.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some(view) = moved
                        .upgrade()
                        .filter(|v| v.read(cx).splitters.drag.is_some())
                    else {
                        return;
                    };
                    view.update(cx, |this, cx| {
                        if e.pressed_button != Some(MouseButton::Left) {
                            this.finish_pane_drag(cx);
                        } else if let Some(drag) = &mut this.splitters.drag {
                            let x = f32::from(e.position.x);
                            let changed = drag.grab.width_at(x) != drag.grab.width_at(drag.x);
                            drag.x = x;
                            if changed {
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }
                    });
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    let Some(view) = view
                        .upgrade()
                        .filter(|v| v.read(cx).splitters.drag.is_some())
                    else {
                        return;
                    };
                    view.update(cx, |this, cx| {
                        if let Some(drag) = &mut this.splitters.drag {
                            drag.x = f32::from(e.position.x);
                        }
                        this.finish_pane_drag(cx);
                        cx.stop_propagation();
                    });
                });
            },
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn dragging_pane(&self) -> bool {
        self.splitters.drag.is_some()
    }
}
