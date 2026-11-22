//! A single undo toast. Its display lifetime is independent of the keyboard undo target.

use std::time::Duration;

use super::{Pace, Track, Wake, millis};
use todo_design::motion as m;

#[derive(Clone, Copy, Debug)]
struct Life {
    remaining: Duration,
    since: Option<Duration>,
}

impl Life {
    fn left(self, now: Duration) -> Duration {
        self.remaining.saturating_sub(
            self.since
                .map_or(Duration::ZERO, |at| now.saturating_sub(at)),
        )
    }
}

#[derive(Debug)]
struct Visible<T> {
    text: T,
    previous: Option<T>,
    alpha: Track,
    y: Track,
    swap: Track,
    life: Option<Life>,
}

#[derive(Debug)]
pub struct Toast<T> {
    visible: Option<Visible<T>>,
    paused: bool,
}

impl<T> Default for Toast<T> {
    fn default() -> Self {
        Self {
            visible: None,
            paused: false,
        }
    }
}

#[derive(Debug)]
pub struct ToastLook<'a, T> {
    pub text: &'a T,
    pub previous: Option<&'a T>,
    pub alpha: f32,
    pub y: f32,
    pub swap: f32,
    pub left: f32,
    pub active: bool,
}

impl<T> Toast<T> {
    pub fn show(&mut self, text: T, now: Duration, pace: Pace) {
        let old = self.visible.take();
        let replace = old.as_ref().is_some_and(|t| t.life.is_some());
        if !replace {
            self.paused = false;
        }
        let old = old.filter(|t| t.life.is_some() || t.alpha.moving(now));
        let life = Some(Life {
            remaining: millis(m::UNDO_WINDOW),
            since: (!self.paused).then_some(now),
        });
        self.visible = Some(if let Some(old) = old {
            let previous = if old.swap.at(now) < 0.5 {
                old.previous.or(Some(old.text))
            } else {
                Some(old.text)
            };
            Visible {
                text,
                previous,
                alpha: if replace {
                    old.alpha
                } else {
                    Track::new(old.alpha.at(now), 1., now, pace.fade(m::TOAST_IN), m::ENTER)
                },
                y: if replace {
                    old.y
                } else {
                    Track::new(old.y.at(now), 0., now, pace.movement(m::TOAST_IN), m::ENTER)
                },
                swap: Track::new(0., 1., now, pace.fade(m::CONTENT_SWAP), m::ENTER),
                life,
            }
        } else {
            Visible {
                text,
                previous: None,
                alpha: Track::new(0., 1., now, pace.fade(m::TOAST_IN), m::ENTER),
                y: Track::new(
                    if pace.reduced { 0. } else { m::TOAST_RISE },
                    0.,
                    now,
                    pace.movement(m::TOAST_IN),
                    m::ENTER,
                ),
                swap: Track::rest(1.),
                life,
            }
        });
    }

    pub fn pause(&mut self, paused: bool, now: Duration) {
        let Some(life) = self.visible.as_mut().and_then(|t| t.life.as_mut()) else {
            return;
        };
        if self.paused != paused {
            self.paused = paused;
            life.remaining = life.left(now);
            life.since = (!paused).then_some(now);
        }
    }

    pub fn dismiss(&mut self, now: Duration, pace: Pace) {
        if let Some(t) = &mut self.visible
            && t.life.take().is_some()
        {
            t.alpha = Track::new(t.alpha.at(now), 0., now, pace.fade(m::TOAST_OUT), m::EXIT);
            t.y = Track::new(
                t.y.at(now),
                if pace.reduced { 0. } else { m::TOAST_DROP },
                now,
                pace.movement(m::TOAST_OUT),
                m::EXIT,
            );
            self.paused = false;
        }
    }

    /// Advances the timer to its actual expiry (even if a frame was late).
    pub fn advance(&mut self, now: Duration, pace: Pace) {
        if let Some(deadline) = self
            .visible
            .as_ref()
            .and_then(|t| t.life)
            .and_then(|l| l.since.map(|at| at + l.remaining))
            && now >= deadline
        {
            self.dismiss(deadline, pace);
        }
        if self
            .visible
            .as_ref()
            .is_some_and(|t| t.life.is_none() && !t.alpha.moving(now))
        {
            self.visible = None;
        }
    }

    pub fn look(&self, now: Duration) -> Option<ToastLook<'_, T>> {
        let t = self.visible.as_ref()?;
        Some(ToastLook {
            text: &t.text,
            previous: t.previous.as_ref().filter(|_| t.swap.moving(now)),
            alpha: t.alpha.at(now),
            y: t.y.at(now),
            swap: t.swap.at(now),
            left: t.life.map_or(0., |l| {
                l.left(now).as_secs_f32() / millis(m::UNDO_WINDOW).as_secs_f32()
            }),
            active: t.life.is_some(),
        })
    }

    pub fn wake(&self, now: Duration) -> Wake {
        if self.visible.as_ref().is_some_and(|t| {
            t.life.is_some_and(|l| l.since.is_some())
                || t.alpha.moving(now)
                || t.y.moving(now)
                || t.swap.moving(now)
        }) {
            Wake::Frame
        } else {
            Wake::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_resets_the_line_without_entering_again() {
        let mut toast = Toast::default();
        let half = millis(m::UNDO_WINDOW / 2);
        toast.show("Completed", Duration::ZERO, Pace::FULL);
        assert_eq!(toast.look(half).expect("visible").left, 0.5);
        toast.show("Moved", half, Pace::FULL);
        let replaced = toast.look(half).expect("replacement");
        assert_eq!((replaced.alpha, replaced.y, replaced.left), (1., 0., 1.));
        assert_eq!(replaced.previous, Some(&"Completed"));
        assert_eq!(
            toast
                .look(half + millis(m::CONTENT_SWAP))
                .expect("settled")
                .previous,
            None
        );
        toast.advance(half + millis(m::UNDO_WINDOW), Pace::FULL);
        assert!(
            !toast
                .look(half + millis(m::UNDO_WINDOW))
                .expect("exiting")
                .active
        );
        toast.advance(half + millis(m::UNDO_WINDOW + m::TOAST_OUT), Pace::FULL);
        assert!(
            toast
                .look(half + millis(m::UNDO_WINDOW + m::TOAST_OUT))
                .is_none()
        );
    }

    #[test]
    fn pause_stops_frames_and_reduced_motion_does_not_shorten_the_timer() {
        let mut toast = Toast::default();
        let half = millis(m::UNDO_WINDOW / 2);
        toast.show((), Duration::ZERO, Pace::REDUCED);
        assert_eq!(toast.look(Duration::ZERO).expect("entering").y, 0.);
        toast.pause(true, half);
        toast.advance(half * 3, Pace::REDUCED);
        assert_eq!(toast.look(half * 3).expect("paused").left, 0.5);
        assert_eq!(toast.wake(half * 3), Wake::Idle);
        toast.pause(false, half * 3);
        toast.advance(half * 4 - millis(1), Pace::REDUCED);
        assert!(
            toast
                .look(half * 4 - millis(1))
                .expect("still active")
                .active
        );
        toast.dismiss(half * 4 - millis(1), Pace::REDUCED);
        assert_eq!(toast.look(half * 4).expect("exiting").y, 0.);
        toast.advance(half * 4 + millis(m::REDUCED_FADE), Pace::REDUCED);
        assert_eq!(toast.wake(half * 4 + millis(m::REDUCED_FADE)), Wake::Idle);
    }
}
