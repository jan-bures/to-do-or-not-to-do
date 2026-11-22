//! Anchored menus and popovers (`docs/design-language.md`, Motion, moment 12). An overlay fades in over
//! `OVERLAY_IN` (`ENTER`) while it moves the last 4 px (`S1`) away from its trigger, and fades out in
//! place over `OVERLAY_OUT` (`EXIT`). GPUI draws no scale, so 1 px borders stay crisp. With reduced
//! motion it only fades, in `REDUCED_FADE`. A closing overlay is paint-only until it has faded. A menu
//! that turns into its next page in place (a picker, a confirmation, the colour swatches) fades the new
//! page in over `CONTENT_SWAP` (`ENTER`), where it stands.

use std::time::Duration;

use super::{Pace, Track, Wake};
use todo_design::motion as m;

/// How an overlay looks at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayLook {
    pub alpha: f32,
    /// How far it still is from its place, towards its trigger: 1 is the whole shift, 0 in place.
    pub shift: f32,
    /// It takes clicks and keys; a closing overlay is only painted.
    pub open: bool,
    /// The opacity of its content, below 1 while a new page fades in.
    pub content: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlay {
    alpha: Track,
    shift: Track,
    open: bool,
    content: Track,
}

impl Overlay {
    /// Open and in place: the screenshot fixtures, and everything drawn without transitions.
    pub fn rest() -> Self {
        Self {
            alpha: Track::rest(1.),
            shift: Track::rest(0.),
            open: true,
            content: Track::rest(1.),
        }
    }

    /// Starts entering now.
    pub fn open(now: Duration, pace: Pace) -> Self {
        Self {
            alpha: Track::new(0., 1., now, pace.fade(m::OVERLAY_IN), m::ENTER),
            shift: Track::new(1., 0., now, pace.movement(m::OVERLAY_IN), m::ENTER),
            open: true,
            content: Track::rest(1.),
        }
    }

    /// Its content changes to another page now: the new page fades in where the old one stood.
    pub fn swap_content(&mut self, now: Duration, pace: Pace) {
        if self.open {
            self.content = Track::new(0., 1., now, pace.fade(m::CONTENT_SWAP), m::ENTER);
        }
    }

    /// Starts leaving now, from wherever it is: it fades where it stands.
    pub fn close(&mut self, now: Duration, pace: Pace) {
        if !self.open {
            return;
        }
        self.open = false;
        self.shift = Track::rest(self.shift.at(now));
        self.alpha.aim(0., now, pace.fade(m::OVERLAY_OUT), m::EXIT);
    }

    pub fn look(&self, now: Duration) -> OverlayLook {
        OverlayLook {
            alpha: self.alpha.at(now),
            shift: self.shift.at(now),
            open: self.open,
            content: self.content.at(now),
        }
    }

    /// Closed and faded out: it can be dropped.
    pub fn gone(&self, now: Duration) -> bool {
        !self.open && self.alpha.at(now) <= 0.
    }

    /// The next frame while it moves; nothing at rest.
    pub fn wake(&self, now: Duration) -> Wake {
        if self.alpha.moving(now) || self.shift.moving(now) || self.content.moving(now) {
            Wake::Frame
        } else {
            Wake::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u32) -> Duration {
        Duration::from_millis(u64::from(n))
    }

    #[test]
    fn it_fades_in_and_settles_into_place() {
        let o = Overlay::open(Duration::ZERO, Pace::FULL);
        let start = o.look(Duration::ZERO);
        assert_eq!((start.alpha, start.shift, start.open), (0., 1., true));
        let half = o.look(ms(m::OVERLAY_IN / 2));
        assert!(half.alpha > 0. && half.alpha < 1.);
        assert!(half.shift > 0. && half.shift < 1.);
        assert_eq!(o.wake(ms(m::OVERLAY_IN / 2)), Wake::Frame);
        let end = o.look(ms(m::OVERLAY_IN));
        assert_eq!((end.alpha, end.shift), (1., 0.));
        // At rest it asks for nothing.
        assert_eq!(o.wake(ms(m::OVERLAY_IN)), Wake::Idle);
    }

    #[test]
    fn closing_fades_in_place_and_is_paint_only() {
        let mut o = Overlay::open(Duration::ZERO, Pace::FULL);
        let at = ms(m::OVERLAY_IN / 2);
        let before = o.look(at);
        o.close(at, Pace::FULL);
        let now = o.look(at);
        // No jump when it reverses part way.
        assert_eq!(now.alpha, before.alpha);
        assert_eq!(now.shift, before.shift);
        assert!(!now.open);
        assert!(!o.gone(at));
        let later = o.look(at + ms(m::OVERLAY_OUT / 2));
        assert!(later.alpha < before.alpha);
        assert_eq!(later.shift, before.shift);
        assert!(o.gone(at + ms(m::OVERLAY_OUT)));
        assert_eq!(o.wake(at + ms(m::OVERLAY_OUT)), Wake::Idle);
    }

    #[test]
    fn reduced_motion_only_fades_briefly() {
        let o = Overlay::open(Duration::ZERO, Pace::REDUCED);
        assert_eq!(o.look(Duration::ZERO).shift, 0.);
        assert_eq!(o.look(ms(m::REDUCED_FADE)).alpha, 1.);
        let mut o = Overlay::rest();
        o.close(Duration::ZERO, Pace::REDUCED);
        assert!(o.gone(ms(m::REDUCED_FADE)));
    }

    #[test]
    fn at_rest_it_is_open_and_in_place() {
        let o = Overlay::rest();
        assert_eq!(
            o.look(Duration::ZERO),
            OverlayLook {
                alpha: 1.,
                shift: 0.,
                open: true,
                content: 1.,
            }
        );
        assert!(!o.gone(Duration::ZERO));
    }

    #[test]
    fn a_new_page_fades_in_where_the_menu_stands() {
        let mut o = Overlay::rest();
        o.swap_content(ms(10), Pace::FULL);
        let start = o.look(ms(10));
        // The menu itself stays put and opaque; only its content starts again from clear.
        assert_eq!((start.alpha, start.shift, start.content), (1., 0., 0.));
        let half = o.look(ms(10 + m::CONTENT_SWAP / 2));
        assert!(half.content > 0. && half.content < 1.);
        assert_eq!(o.wake(ms(10 + m::CONTENT_SWAP / 2)), Wake::Frame);
        assert_eq!(o.look(ms(10 + m::CONTENT_SWAP)).content, 1.);
        assert_eq!(o.wake(ms(10 + m::CONTENT_SWAP)), Wake::Idle);
        // Reduced motion keeps it a short fade; a closing menu doesn't swap.
        let mut r = Overlay::rest();
        r.swap_content(Duration::ZERO, Pace::REDUCED);
        assert_eq!(r.look(ms(m::REDUCED_FADE)).content, 1.);
        let mut closing = Overlay::rest();
        closing.close(Duration::ZERO, Pace::FULL);
        closing.swap_content(Duration::ZERO, Pace::FULL);
        assert_eq!(closing.look(Duration::ZERO).content, 1.);
    }
}
