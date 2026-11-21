//! A vertical scrollbar for a GPUI scroll handle. Like Zed's scrollbar, it reads the
//! handle after the scroller has laid out and maps the available thumb travel to its offset.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, DispatchPhase, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, Styled, UniformListScrollHandle, canvas,
    fill, point, px, size,
};

use crate::theme::{Palette, chrome, desktop, with_alpha};

#[derive(Default)]
pub struct Scrollbar {
    // The pointer's distance from the top of the thumb, if dragging. Keep it across paints.
    grab: Rc<Cell<Option<Pixels>>>,
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
    pub fn for_list(
        &self,
        list: &UniformListScrollHandle,
        p: &Palette,
    ) -> impl IntoElement + use<> {
        self.overlay(&list.0.borrow().base_handle, p)
    }

    /// Append after the scroll view, in its unscrolled parent. The edge gutter must be at least
    /// `SCROLLBAR_INSET + SCROLLBAR_HOVER_WIDTH` wide, so the track never covers row content.
    pub fn overlay(&self, handle: &ScrollHandle, p: &Palette) -> impl IntoElement + use<> {
        let handle = handle.clone();
        let grab = self.grab.clone();
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
            move |_, state, window, _| {
                let Some((handle, grab, hitbox, thumb, viewport)) = state else {
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
                let alpha = if hot {
                    chrome::SCROLLBAR_HOVER_ALPHA
                } else {
                    chrome::SCROLLBAR_ALPHA
                };
                let bounds = Bounds::new(
                    point(track.right() - width, track.top() + thumb.top),
                    size(width, thumb.length),
                );
                window.paint_quad(fill(bounds, with_alpha(ink, alpha)).corner_radii(width / 2.));

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
