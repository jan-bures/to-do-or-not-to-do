//! Pure timing for sidebar disclosure/selection and overlay scrollbars.

use super::{Pace, Track, Wake, millis};
use crate::{model::RoomKey, theme::motion as m};
use std::{collections::HashMap, time::Duration};
use todo_core::ids::CollectionId;

#[derive(Clone, Debug, Default)]
pub struct SidebarMotion {
    groups: HashMap<CollectionId, Disclosure>,
    rings: HashMap<RoomKey, Track>,
}

#[derive(Clone, Copy, Debug)]
struct Disclosure {
    rows: Track,
    chevron: Track,
}

impl SidebarMotion {
    pub fn disclose(&mut self, id: CollectionId, open: bool, now: Duration, pace: Pace) {
        let from = f32::from(!open);
        let group = self.groups.entry(id).or_insert(Disclosure {
            rows: Track::rest(from),
            chevron: Track::rest(from),
        });
        let to = f32::from(open);
        group.rows.aim(
            to,
            now,
            pace.movement(m::PANEL),
            if open { m::ENTER } else { m::EXIT },
        );
        group
            .chevron
            .aim(to, now, pace.movement(m::PANEL), m::STANDARD);
    }

    /// Both numbers go from 0 (closed) to 1 (open), with independent curves.
    pub fn group(&self, id: CollectionId, open: bool, now: Duration) -> (f32, f32) {
        self.groups.get(&id).map_or_else(
            || (f32::from(open), f32::from(open)),
            |g| (g.rows.at(now), g.chevron.at(now)),
        )
    }

    pub fn select(&mut self, from: RoomKey, to: RoomKey, now: Duration, pace: Pace) {
        if from == to {
            return;
        }
        self.rings
            .entry(from)
            .or_insert_with(|| Track::rest(1.))
            .aim(0., now, pace.fade(m::HOVER), m::STANDARD);
        self.rings.entry(to).or_insert_with(|| Track::rest(0.)).aim(
            1.,
            now,
            pace.fade(m::HOVER),
            m::STANDARD,
        );
    }

    pub fn ring(&self, key: RoomKey, selected: bool, now: Duration) -> f32 {
        self.rings
            .get(&key)
            .map_or(f32::from(selected), |t| t.at(now))
    }

    pub fn prune(&mut self, now: Duration) {
        self.groups
            .retain(|_, g| g.rows.moving(now) || g.chevron.moving(now));
        self.rings.retain(|_, t| t.moving(now));
    }

    pub(super) fn wake(&self, now: Duration) -> Wake {
        if self
            .groups
            .values()
            .any(|g| g.rows.moving(now) || g.chevron.moving(now))
            || self.rings.values().any(|t| t.moving(now))
        {
            Wake::Frame
        } else {
            Wake::Idle
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollbarLook {
    pub alpha: f32,
    pub emphasis: f32,
    pub wake: Wake,
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollbarMotion {
    seen_offset: Option<f32>,
    hot: bool,
    hold_until: Option<Duration>,
    alpha: Track,
    emphasis: Track,
}

impl Default for ScrollbarMotion {
    fn default() -> Self {
        Self {
            seen_offset: None,
            hot: false,
            hold_until: None,
            alpha: Track::rest(0.),
            emphasis: Track::rest(0.),
        }
    }
}

impl ScrollbarMotion {
    pub fn sample(
        &mut self,
        now: Duration,
        offset: f32,
        hot: bool,
        always: bool,
        pace: Pace,
    ) -> ScrollbarLook {
        let old_offset = self.seen_offset.replace(offset).unwrap_or(0.);
        if old_offset != offset || (self.hot && !hot) {
            self.hold_until = Some(now + millis(m::SCROLLBAR_HOLD));
        }
        self.hot = hot;
        let holding = self.hold_until.filter(|&at| now < at);
        if always {
            self.alpha = Track::rest(1.);
        } else if hot || holding.is_some() {
            self.alpha.aim(1., now, pace.fade(m::HOVER), m::ENTER);
        } else {
            let start = self.hold_until.take().unwrap_or(now);
            self.alpha
                .aim(0., start, pace.fade(m::SCROLLBAR_OUT), m::EXIT);
        }
        self.emphasis
            .aim(f32::from(hot), now, pace.fade(m::HOVER), m::STANDARD);
        let wake = if self.alpha.moving(now) || self.emphasis.moving(now) {
            Wake::Frame
        } else if always || hot {
            Wake::Idle
        } else {
            holding.map_or(Wake::Idle, Wake::At)
        };
        ScrollbarLook {
            alpha: self.alpha.at(now),
            emphasis: self.emphasis.at(now),
            wake,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Smart;
    use uuid::Uuid;

    #[test]
    fn disclosure_reverses_from_the_visible_height_and_angle() {
        let id = CollectionId::from_uuid(Uuid::from_u128(1));
        let mut s = SidebarMotion::default();
        s.disclose(id, false, Duration::ZERO, Pace::FULL);
        let at = millis(m::PANEL / 2);
        let partial = s.group(id, false, at);
        assert!((0. ..1.).contains(&partial.0));
        assert_ne!(partial.0, partial.1);
        s.disclose(id, true, at, Pace::FULL);
        assert_eq!(s.group(id, true, at), partial);
        assert_eq!(s.group(id, true, at + millis(m::PANEL)), (1., 1.));
        s.disclose(id, false, at + millis(m::PANEL), Pace::REDUCED);
        assert_eq!(s.group(id, false, at + millis(m::PANEL)), (0., 0.));
        assert_eq!(s.wake(at + millis(m::PANEL)), Wake::Idle);
    }

    #[test]
    fn selection_crossfades_between_rows_and_retargets() {
        let a = RoomKey::Smart(Smart::MyDay);
        let b = RoomKey::Smart(Smart::Important);
        let mut s = SidebarMotion::default();
        s.select(a, b, Duration::ZERO, Pace::FULL);
        let at = millis(m::HOVER / 2);
        let (old, new) = (s.ring(a, false, at), s.ring(b, true, at));
        assert!(old > 0. && new > 0.);
        assert!((old + new - 1.).abs() < 1e-6);
        s.select(b, a, at, Pace::FULL);
        assert_eq!((s.ring(a, true, at), s.ring(b, false, at)), (old, new));
    }

    #[test]
    fn scrollbar_holds_without_frames_and_can_reappear_during_the_fade() {
        let mut s = ScrollbarMotion::default();
        let (zero, pace) = (Duration::ZERO, Pace::FULL);
        assert_eq!(s.sample(zero, 0., false, false, pace).wake, Wake::Idle);
        assert_eq!(s.sample(zero, 10., false, false, pace).wake, Wake::Frame);
        let due = millis(m::SCROLLBAR_HOLD);
        assert_eq!(
            s.sample(millis(m::HOVER), 10., false, false, pace).wake,
            Wake::At(due)
        );
        let mid = due + millis(m::SCROLLBAR_OUT / 2);
        let fading = s.sample(mid, 10., false, false, pace);
        assert!(fading.alpha > 0. && fading.alpha < 1.);
        assert_eq!(s.sample(mid, 15., false, false, pace).alpha, fading.alpha);
        assert_eq!(
            s.sample(mid + millis(m::HOVER), 15., false, false, pace)
                .alpha,
            1.
        );
    }

    #[test]
    fn scrollbar_respects_reduced_motion_and_always_visible() {
        let mut s = ScrollbarMotion::default();
        let pace = Pace::REDUCED;
        s.sample(Duration::ZERO, 0., true, false, pace);
        let rest = millis(m::HOVER);
        assert_eq!(s.sample(rest, 0., true, false, pace).wake, Wake::Idle);
        let due = rest + millis(m::SCROLLBAR_HOLD);
        assert_eq!(s.sample(rest, 0., false, false, pace).wake, Wake::Frame);
        assert_eq!(
            s.sample(rest * 2, 0., false, false, pace).wake,
            Wake::At(due)
        );
        assert_eq!(
            s.sample(due + millis(m::REDUCED_FADE), 0., false, false, pace)
                .alpha,
            0.
        );
        assert_eq!(s.sample(due * 2, 0., false, true, pace).alpha, 1.);
        assert_eq!(s.sample(due * 2, 0., false, true, pace).wake, Wake::Idle);
    }
}
