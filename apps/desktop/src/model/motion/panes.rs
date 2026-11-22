//! Room and detail transitions. Sample the displayed value when retargeting rather than restarting
//! from the old intent. The view holds the outgoing content; this model never holds a live editor.

use super::{Pace, Wake, millis, mix, stretch};
use crate::theme::{Hue, motion as m, space};
use std::time::Duration;
use todo_core::ids::TaskId;

/// A scalar that can reverse or retarget without a discontinuity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    from: f32,
    to: f32,
    start: Duration,
    span: Duration,
    curve: m::CubicBezier,
}

impl Track {
    pub fn rest(value: f32) -> Self {
        Self::new(value, value, Duration::ZERO, Duration::ZERO, m::STANDARD)
    }

    pub fn new(from: f32, to: f32, start: Duration, span: Duration, curve: m::CubicBezier) -> Self {
        Self {
            from,
            to,
            start,
            span,
            curve,
        }
    }

    pub fn at(self, now: Duration) -> f32 {
        mix(
            self.from,
            self.to,
            self.curve.ease(stretch(now, self.start, self.span)),
        )
    }

    pub fn moving(self, now: Duration) -> bool {
        self.from != self.to && now < self.start + self.span
    }

    pub fn target(self) -> f32 {
        self.to
    }

    pub fn aim(&mut self, to: f32, now: Duration, span: Duration, curve: m::CubicBezier) {
        if self.to != to {
            *self = Self::new(self.at(now), to, now, span, curve);
        }
    }
}

/// The signed shortest arc between two hues, including a switch across red at 0°.
fn hue_delta(from: Hue, to: Hue) -> Hue {
    (to - from + 180.).rem_euclid(360.) - 180.
}

#[derive(Clone, Copy, Debug)]
struct HueTravel {
    from: Hue,
    to: Hue,
    track: Track,
}

impl HueTravel {
    fn at(self, now: Duration) -> Hue {
        let t = self.track.at(now);
        if t == 1. {
            self.to
        } else {
            (self.from + hue_delta(self.from, self.to) * f64::from(t)).rem_euclid(360.)
        }
    }
}

/// The alpha and vertical offsets of the two layers. The outgoing layer is always paint-only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwapLook {
    pub outgoing: f32,
    pub incoming: f32,
    pub outgoing_y: f32,
    pub incoming_y: f32,
}

impl SwapLook {
    const REST: Self = Self {
        outgoing: 0.,
        incoming: 1.,
        outgoing_y: 0.,
        incoming_y: 0.,
    };
}

#[derive(Clone, Copy, Debug)]
struct Swap {
    start: Duration,
    pace: Pace,
}

impl Swap {
    fn list(self, now: Duration) -> SwapLook {
        let incoming = m::ENTER.ease(stretch(now, self.start, self.pace.fade(m::CONTENT_SWAP)));
        SwapLook {
            outgoing: 1. - incoming,
            incoming,
            ..SwapLook::REST
        }
    }

    fn title(self, now: Duration) -> SwapLook {
        let span = self.pace.fade(m::CONTENT_SWAP);
        let delay = if self.pace.reduced {
            Duration::ZERO
        } else {
            span - millis(m::ROOM_TITLE_OVERLAP)
        };
        let out = m::EXIT.ease(stretch(now, self.start, span));
        let inc = m::ENTER.ease(stretch(now, self.start + delay, span));
        let glide = if self.pace.reduced { 0. } else { space::S1 };
        SwapLook {
            outgoing: 1. - out,
            incoming: inc,
            outgoing_y: if out == 1. { 0. } else { -glide * out },
            incoming_y: glide * (1. - inc),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Navigation {
    hue: Option<HueTravel>,
    swap: Option<Swap>,
}

impl Navigation {
    pub fn switch(&mut self, from: Hue, to: Hue, now: Duration, pace: Pace) {
        self.aim_hue(from, to, now, pace);
        self.swap = Some(Swap { start: now, pace });
    }

    pub fn aim_hue(&mut self, from: Hue, to: Hue, now: Duration, pace: Pace) {
        let from = self.hue(now, from);
        self.hue = (from != to).then(|| HueTravel {
            from,
            to,
            track: Track::new(0., 1., now, pace.fade(m::ROOM_HUE), m::SUN),
        });
    }

    pub fn hue(self, now: Duration, rest: Hue) -> Hue {
        self.hue.map_or(rest, |h| h.at(now))
    }

    pub fn title(self, now: Duration) -> SwapLook {
        self.swap.map_or(SwapLook::REST, |s| s.title(now))
    }

    pub fn list(self, now: Duration) -> SwapLook {
        self.swap.map_or(SwapLook::REST, |s| s.list(now))
    }

    pub(super) fn wake(self, now: Duration) -> Wake {
        if self.hue.is_some_and(|h| h.track.moving(now))
            || self.title(now) != SwapLook::REST
            || self.list(now) != SwapLook::REST
        {
            Wake::Frame
        } else {
            Wake::Idle
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailLook {
    /// Opacity of the entering/leaving task surface (and, in a narrow window, its scrim).
    pub task: f32,
    pub task_x: f32,
    /// Opacity of the wide column's empty placeholder.
    pub placeholder: f32,
    /// Opacity of the body, independent of the anchored action bar.
    pub body: f32,
    /// Keep a wide column at full width during its exit, then release the slot together.
    pub docked: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DetailMotion {
    task: Option<TaskId>,
    present: Track,
    x: Track,
    placeholder: Track,
    body: Track,
    last_switch: Option<Duration>,
    dock: Track,
}

impl Default for DetailMotion {
    fn default() -> Self {
        Self::rest(None, true)
    }
}

impl DetailMotion {
    pub fn rest(task: Option<TaskId>, docked: bool) -> Self {
        Self {
            task,
            present: Track::rest(f32::from(task.is_some())),
            x: Track::rest(0.),
            placeholder: Track::rest(f32::from(task.is_none() && docked)),
            body: Track::rest(1.),
            last_switch: None,
            dock: Track::rest(f32::from(docked)),
        }
    }

    pub fn aim(
        &mut self,
        task: Option<TaskId>,
        docked: bool,
        narrow: bool,
        now: Duration,
        pace: Pace,
    ) {
        let (open, close, slide) = if narrow {
            (m::OVERLAY_IN, m::OVERLAY_OUT, space::S6)
        } else {
            (m::PANEL, m::PANEL, space::S2)
        };
        let shown = task.is_some();
        let was_shown = self.task.is_some();
        if was_shown != shown {
            let (span, curve) = if shown {
                (open, m::ENTER)
            } else {
                (close, m::EXIT)
            };
            if shown && self.present.at(now) == 0. {
                self.x = Track::rest(slide);
            }
            self.present
                .aim(f32::from(shown), now, pace.fade(span), curve);
            self.x = if pace.reduced {
                Track::rest(0.)
            } else {
                Track::new(
                    self.x.at(now),
                    if shown { 0. } else { slide },
                    now,
                    pace.movement(span),
                    curve,
                )
            };
        }
        if task != self.task {
            // Repeated switches within the content-swap window stay solid, not just every other one.
            let switching = was_shown && shown;
            let repeated = self
                .last_switch
                .is_some_and(|at| now < at + millis(m::CONTENT_SWAP));
            self.last_switch = switching.then_some(now);
            self.body = if switching && !repeated {
                Track::new(
                    m::DETAIL_SWAP_FROM,
                    1.,
                    now,
                    pace.fade(m::CONTENT_SWAP),
                    m::STANDARD,
                )
            } else {
                Track::rest(1.)
            };
            self.task = task;
        }
        let placeholder = docked && !shown;
        self.placeholder.aim(
            f32::from(placeholder),
            now,
            pace.fade(if docked { m::CONTENT_SWAP } else { close }),
            if placeholder { m::ENTER } else { m::EXIT },
        );
        // Keep the width fixed throughout the fade. The slot only needs to wait when leaving.
        if docked {
            self.dock = Track::rest(1.);
        } else {
            self.dock.aim(0., now, pace.fade(close), m::EXIT);
        }
    }

    pub fn look(self, now: Duration) -> DetailLook {
        DetailLook {
            task: self.present.at(now),
            task_x: self.x.at(now),
            placeholder: self.placeholder.at(now),
            body: self.body.at(now),
            docked: self.dock.at(now) > 0.,
        }
    }

    pub(super) fn wake(self, now: Duration) -> Wake {
        if [self.present, self.x, self.placeholder, self.body, self.dock]
            .into_iter()
            .any(|x| x.moving(now))
        {
            Wake::Frame
        } else {
            Wake::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::ids::TaskId;
    use uuid::Uuid;

    fn task(n: u128) -> Option<TaskId> {
        Some(TaskId::from_uuid(Uuid::from_u128(n)))
    }

    #[test]
    fn shortest_arc_retargets_from_the_displayed_hue() {
        let mut nav = Navigation::default();
        nav.switch(350., 20., Duration::ZERO, Pace::FULL);
        let half = millis(m::ROOM_HUE / 2);
        assert_eq!(nav.hue(half, 20.), 5.);
        nav.switch(20., 335., half, Pace::FULL);
        assert_eq!(nav.hue(half, 335.), 5.);
        assert_eq!(nav.hue(half * 2, 335.), 350.);
        assert_eq!(nav.hue(half * 3, 335.), 335.);
        assert_eq!(nav.wake(half * 3), Wake::Idle);
    }

    #[test]
    fn title_layers_overlap_but_reduced_motion_does_not_translate() {
        let mut nav = Navigation::default();
        nav.switch(0., 90., Duration::ZERO, Pace::FULL);
        assert_eq!(nav.list(Duration::ZERO).incoming, 0.);
        let title = nav.title(millis(m::CONTENT_SWAP - m::ROOM_TITLE_OVERLAP / 2));
        assert!(title.incoming > 0. && title.outgoing > 0.);
        assert!(title.incoming_y > 0. && title.outgoing_y < 0.);
        nav.switch(90., 0., Duration::ZERO, Pace::REDUCED);
        let title = nav.title(millis(m::REDUCED_FADE / 2));
        assert!(title.incoming > 0. && title.outgoing > 0.);
        assert_eq!((title.outgoing_y, title.incoming_y), (0., 0.));
        assert_eq!(nav.wake(millis(m::REDUCED_FADE)), Wake::Idle);
    }

    #[test]
    fn dock_keeps_its_width_until_the_dusk_exit_is_over() {
        let mut d = DetailMotion::rest(task(1), true);
        d.aim(None, false, false, Duration::ZERO, Pace::FULL);
        let half = millis(m::PANEL / 2);
        let look = d.look(half);
        assert!(look.docked && look.task > 0. && look.task < 1. && look.task_x > 0.);
        assert_eq!(look.placeholder, 0.);
        assert!(!d.look(half * 2).docked);
        assert_eq!(d.wake(half * 2), Wake::Idle);
    }

    #[test]
    fn interrupted_overlay_moves_from_its_current_look() {
        let mut d = DetailMotion::rest(None, false);
        let half = millis(m::OVERLAY_IN / 2);
        d.aim(task(1), false, true, Duration::ZERO, Pace::FULL);
        let before = d.look(half);
        d.aim(None, false, true, half, Pace::FULL);
        assert_eq!(d.look(half), before);
        assert_eq!(d.wake(half + millis(m::OVERLAY_OUT)), Wake::Idle);
    }

    #[test]
    fn rapid_switch_snaps_the_body_and_never_moves_the_bar() {
        let mut d = DetailMotion::rest(task(1), true);
        d.aim(task(2), true, false, Duration::ZERO, Pace::REDUCED);
        assert_eq!(d.look(Duration::ZERO).body, m::DETAIL_SWAP_FROM);
        let half = millis(m::REDUCED_FADE / 2);
        d.aim(task(3), true, false, half, Pace::REDUCED);
        let look = d.look(half);
        assert_eq!((look.body, look.task, look.task_x), (1., 1., 0.));
        assert_eq!(d.wake(half), Wake::Idle);
        d.aim(task(4), true, false, half * 2, Pace::REDUCED);
        assert_eq!(d.look(half * 2).body, 1.);
    }
}
