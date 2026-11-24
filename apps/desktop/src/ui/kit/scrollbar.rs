//! A vertical scrollbar for a GPUI scroll handle. Like Zed's scrollbar, it reads the
//! handle after the scroller has laid out and maps the available thumb travel to its offset.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, DispatchPhase, Div, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, SharedString, Stateful, Styled, Task,
    UniformListScrollHandle, canvas, div, fill, point, prelude::*, px, size,
};

use crate::model::motion::{Pace, ScrollbarMotion, Wake, mix};
use crate::platform::win;
use crate::theme::{Palette, chrome, desktop, with_alpha};

pub const SCROLLBAR_GUTTER: f32 = desktop::SCROLLBAR_INSET + desktop::SCROLLBAR_HOVER_WIDTH;

#[derive(Default)]
pub struct ScrollPane {
    pub handle: ScrollHandle,
    pub bar: Scrollbar,
}

impl ScrollPane {
    pub fn viewport(&self, id: impl Into<SharedString>) -> Stateful<Div> {
        div()
            .id(id.into())
            .overflow_y_scroll()
            .track_scroll(&self.handle)
            // GPUI dispatches bubble listeners in reverse paint order, so its built-in
            // scroll listener has already run here. Consume the event even at the edges.
            .on_scroll(|_, _, cx| cx.stop_propagation())
    }

    pub fn reset_to_top(&self) {
        self.bar.reset_to_top(&self.handle);
    }
}

#[derive(Default)]
pub struct Scrollbar {
    // The pointer's distance from the top of the thumb, if dragging. Keep it across paints.
    grab: Rc<Cell<Option<Pixels>>>,
    fade: Rc<RefCell<Fade>>,
    frozen_alpha: Option<f32>,
}

#[derive(Default)]
struct Fade {
    epoch: Option<Instant>,
    motion: ScrollbarMotion,
    alarm: Option<(Duration, Task<()>)>,
}

impl Fade {
    fn alarm_fired(&mut self, at: Duration) -> bool {
        if let Some((_, task)) = self.alarm.take_if(|(due, _)| *due == at) {
            // The alarm may be a little early; clear its slot before a refresh can re-arm it.
            task.detach();
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Thumb {
    top: Pixels,
    length: Pixels,
    travel: Pixels,
    max_scroll: Pixels,
}

impl Thumb {
    fn new(viewport: Pixels, track: Pixels, max_scroll: Pixels, scroll: Pixels) -> Option<Self> {
        if viewport <= px(0.) || track <= px(0.) || max_scroll <= px(0.) {
            return None;
        }
        let min_thumb = px(desktop::SCROLLBAR_MIN_THUMB).min(track / 2.);
        let length = (track * (viewport / (viewport + max_scroll))).max(min_thumb);
        let travel = track - length;
        Some(Self {
            top: travel * (scroll.clamp(px(0.), max_scroll) / max_scroll),
            length,
            travel,
            max_scroll,
        })
    }

    fn scroll_for_top(self, top: Pixels) -> Pixels {
        if self.travel <= px(0.) {
            return px(0.);
        }
        self.max_scroll * (top.clamp(px(0.), self.travel) / self.travel)
    }

    fn page(self, pointer: Pixels, scroll: Pixels, viewport: Pixels) -> Pixels {
        let delta = if pointer < self.top {
            -viewport
        } else {
            viewport
        };
        (scroll + delta).clamp(px(0.), self.max_scroll)
    }
}

impl Scrollbar {
    /// A new pane starts at the top, not with an apparent scroll from the old content's offset.
    pub fn reset_to_top(&self, handle: &ScrollHandle) {
        handle.set_offset(point(px(0.), px(0.)));
        self.grab.set(None);
        *self.fade.borrow_mut() = Fade::default();
    }

    /// Snapshot the ordinary scroll/hold/fade path, without reading the OS preference or a clock.
    pub fn freeze_fade(&mut self, at: u32) {
        let mut motion = ScrollbarMotion::default();
        motion.sample(Duration::ZERO, 1., false, false, Pace::FULL);
        self.frozen_alpha = Some(
            motion
                .sample(
                    Duration::from_millis(at.into()),
                    1.,
                    false,
                    false,
                    Pace::FULL,
                )
                .alpha,
        );
    }

    pub fn for_list(
        &self,
        list: &UniformListScrollHandle,
        p: &Palette,
        animate: bool,
    ) -> impl IntoElement + use<> {
        self.overlay(&list.0.borrow().base_handle, p, animate)
    }

    /// Append after the scroll view, in its unscrolled parent. The edge gutter must be at least
    /// `SCROLLBAR_INSET + SCROLLBAR_HOVER_WIDTH` wide, so the track never covers row content.
    pub fn overlay(
        &self,
        handle: &ScrollHandle,
        p: &Palette,
        animate: bool,
    ) -> impl IntoElement + use<> {
        self.overlay_with_visibility(handle, p, animate, false)
    }

    pub fn overlay_pinned(
        &self,
        handle: &ScrollHandle,
        p: &Palette,
        animate: bool,
    ) -> impl IntoElement + use<> {
        self.overlay_with_visibility(handle, p, animate, true)
    }

    fn overlay_with_visibility(
        &self,
        handle: &ScrollHandle,
        p: &Palette,
        animate: bool,
        pinned: bool,
    ) -> impl IntoElement + use<> {
        let handle = handle.clone();
        let grab = self.grab.clone();
        let fade_state = self.fade.clone();
        let frozen_alpha = self.frozen_alpha.unwrap_or(1.);
        let ink = p.ink;
        let inset = px(desktop::SCROLLBAR_INSET);
        let wide = px(desktop::SCROLLBAR_HOVER_WIDTH);
        canvas(
            move |_, window, _| {
                let viewport = handle.bounds();
                let height = viewport.size.height - inset * 2.;
                let thumb = Thumb::new(
                    viewport.size.height,
                    height,
                    handle.max_offset().y,
                    -handle.offset().y,
                );
                let Some(thumb) = thumb else {
                    grab.set(None);
                    return None;
                };
                let track = Bounds::new(
                    point(viewport.right() - inset - wide, viewport.top() + inset),
                    size(wide, height),
                );
                // Wheel events still reach the underlying scroller, even over the thumb.
                let hitbox = window.insert_hitbox(track, HitboxBehavior::BlockMouseExceptScroll);
                Some((handle, grab, hitbox, thumb, viewport.size.height))
            },
            move |_, state, window, cx| {
                let Some((handle, grab, hitbox, thumb, viewport)) = state else {
                    if animate {
                        *fade_state.borrow_mut() = Fade::default();
                    }
                    return;
                };
                let track = hitbox.bounds;
                let hovered = hitbox.is_hovered(window);
                let hot = hovered || grab.get().is_some();
                let width = if hot {
                    wide
                } else {
                    px(desktop::SCROLLBAR_WIDTH)
                };
                let (visible, emphasis) = if animate {
                    let mut fade = fade_state.borrow_mut();
                    let now = fade.epoch.get_or_insert_with(Instant::now).elapsed();
                    let pace = if win::reduced_motion() {
                        Pace::REDUCED
                    } else {
                        Pace::FULL
                    };
                    let sample = fade.motion.sample(
                        now,
                        f32::from(handle.offset().y),
                        hot,
                        pinned || !cx.should_auto_hide_scrollbars(),
                        pace,
                    );
                    match sample.wake {
                        Wake::Frame => {
                            fade.alarm = None;
                            window.request_animation_frame();
                        }
                        Wake::At(at) if fade.alarm.as_ref().is_none_or(|(old, _)| *old != at) => {
                            let timer = cx.background_executor().timer(at.saturating_sub(now));
                            let fade_handle = Rc::downgrade(&fade_state);
                            fade.alarm = Some((
                                at,
                                window.spawn(cx, async move |cx| {
                                    timer.await;
                                    if fade_handle
                                        .upgrade()
                                        .is_some_and(|fade| fade.borrow_mut().alarm_fired(at))
                                    {
                                        let _ = cx.update(|window, _| window.refresh());
                                    }
                                }),
                            ));
                        }
                        Wake::Idle => fade.alarm = None,
                        Wake::At(_) => {}
                    }
                    (sample.alpha, sample.emphasis)
                } else {
                    // Fixtures remain deterministic, regardless of the machine's settings or clock.
                    (frozen_alpha, 0.)
                };
                let alpha = visible
                    * mix(
                        chrome::SCROLLBAR_ALPHA,
                        chrome::SCROLLBAR_HOVER_ALPHA,
                        emphasis,
                    );
                let bounds = Bounds::new(
                    point(track.right() - width, track.top() + thumb.top),
                    size(width, thumb.length),
                );
                if alpha > 0. {
                    window
                        .paint_quad(fill(bounds, with_alpha(ink, alpha)).corner_radii(width / 2.));
                }

                let down_handle = handle.clone();
                let down_grab = grab.clone();
                let down_hitbox = hitbox.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !down_hitbox.is_hovered(window)
                    {
                        return;
                    }
                    let pointer = event.position.y - track.top();
                    if pointer >= thumb.top && pointer <= thumb.top + thumb.length {
                        down_grab.set(Some(pointer - thumb.top));
                    } else {
                        let offset = down_handle.offset();
                        down_handle
                            .set_offset(point(offset.x, -thumb.page(pointer, -offset.y, viewport)));
                    }
                    window.refresh();
                    cx.stop_propagation();
                });
                let move_grab = grab.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if let Some(grabbed_at) = move_grab.get() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            let scroll =
                                thumb.scroll_for_top(event.position.y - track.top() - grabbed_at);
                            handle.set_offset(point(handle.offset().x, -scroll));
                            cx.stop_propagation();
                        } else {
                            move_grab.set(None);
                        }
                        window.refresh();
                    } else if hovered != hitbox.is_hovered(window) {
                        window.refresh();
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture
                        && event.button == MouseButton::Left
                        && grab.take().is_some()
                    {
                        window.refresh();
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .right_0()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_early_alarm_can_rearm_for_the_same_deadline() {
        let mut fade = Fade::default();
        let pace = Pace::FULL;
        fade.motion.sample(Duration::ZERO, 10., false, false, pace);
        let Wake::At(due) = fade
            .motion
            .sample(Duration::from_millis(200), 10., false, false, pace)
            .wake
        else {
            panic!("expected a hold deadline");
        };
        fade.alarm = Some((due, Task::ready(())));
        let early = due - Duration::from_millis(1);
        assert!(!fade.alarm_fired(early)); // A stale alarm must not clear this one.
        assert!(fade.alarm_fired(due));
        assert!(fade.alarm.is_none());
        assert_eq!(
            fade.motion.sample(early, 10., false, false, pace).wake,
            Wake::At(due)
        );
        assert_eq!(
            fade.motion.sample(due, 10., false, false, pace).wake,
            Wake::Frame
        );
        let settled = fade
            .motion
            .sample(due + Duration::from_secs(1), 10., false, false, pace);
        assert_eq!((settled.alpha, settled.wake), (0., Wake::Idle));
    }

    #[test]
    fn resetting_to_top_does_not_reveal_the_thumb() {
        let bar = Scrollbar::default();
        let handle = ScrollHandle::new();
        let pace = Pace::FULL;
        let now = Duration::from_secs(2);
        handle.set_offset(point(px(0.), px(-10.)));
        bar.fade
            .borrow_mut()
            .motion
            .sample(Duration::ZERO, -10., false, false, pace);
        assert_eq!(
            bar.fade
                .borrow_mut()
                .motion
                .sample(now, -10., false, false, pace)
                .alpha,
            0.
        );
        bar.grab.set(Some(px(3.)));
        bar.reset_to_top(&handle);
        let offset = f32::from(handle.offset().y);
        assert_eq!(offset, 0.);
        assert_eq!(bar.grab.get(), None);
        let fade = &mut *bar.fade.borrow_mut();
        let reset = fade
            .motion
            .sample(Duration::ZERO, offset, false, false, pace);
        assert_eq!((reset.alpha, reset.wake), (0., Wake::Idle));
        assert_eq!(
            fade.motion.sample(now, -10., false, false, pace).wake,
            Wake::Frame
        );
    }

    #[test]
    fn thumb_is_only_present_on_overflow() {
        assert!(Thumb::new(px(300.), px(296.), px(0.), px(0.)).is_none());
        assert!(Thumb::new(px(4.), px(0.), px(40.), px(0.)).is_none());
    }

    #[test]
    fn thumb_size_and_position_are_proportional() {
        let thumb = Thumb::new(px(300.), px(296.), px(900.), px(450.)).expect("overflow");
        assert_eq!(thumb.length, px(74.));
        assert_eq!(thumb.top, px(111.));
        assert_eq!(thumb.scroll_for_top(thumb.top), px(450.));
        let huge = Thumb::new(px(300.), px(296.), px(30_000.), px(0.)).expect("overflow");
        assert_eq!(huge.length, px(desktop::SCROLLBAR_MIN_THUMB));
    }

    #[test]
    fn thumb_and_drag_clamp_to_both_ends() {
        let at_start = Thumb::new(px(300.), px(296.), px(900.), px(-25.)).expect("overflow");
        assert_eq!(at_start.top, px(0.));
        let at_end = Thumb::new(px(300.), px(296.), px(900.), px(1000.)).expect("overflow");
        assert_eq!(at_end.top + at_end.length, px(296.));
        assert_eq!(at_end.scroll_for_top(px(-10.)), px(0.));
        assert_eq!(at_end.scroll_for_top(px(300.)), px(900.));
    }

    #[test]
    fn track_clicks_move_one_viewport_and_clamp() {
        let thumb = Thumb::new(px(300.), px(296.), px(900.), px(450.)).expect("overflow");
        assert_eq!(thumb.page(px(5.), px(450.), px(300.)), px(150.));
        assert_eq!(thumb.page(px(290.), px(450.), px(300.)), px(750.));
        assert_eq!(thumb.page(px(5.), px(50.), px(300.)), px(0.));
        assert_eq!(thumb.page(px(290.), px(850.), px(300.)), px(900.));
    }
}
