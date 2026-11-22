//! The completion choreography and the sun's travel, as plain functions of a clock.
//!
//! The view keeps one [`Motion`] per window and a monotonic clock. Toggling a task starts that row's
//! choreography ([`Motion::complete`], [`Motion::reopen`]); each frame samples [`Motion::row`] and
//! [`Motion::sun`] at the frame's time, calls [`Motion::advance`] to learn which rows have finished leaving
//! (the only moments that change what the list holds), and asks [`Motion::wake`] when it needs the next
//! frame: every frame while something moves, one timer while a row only holds still, and nothing at rest.
//!
//! Completing a task (`docs/design-language.md`, Motion, moment 1):
//!
//! 1. the disc fills and pops (`CHECK_FILL`, `POP`); the tick draws, the title dims and is struck through
//!    (`CHECK_TICK`, `STANDARD`, after `CHECK_TICK_DELAY`);
//! 2. the row holds still (`COMPLETE_HOLD`);
//! 3. it fades and collapses (`ROW_COLLAPSE`, `EXIT`), and only then is it released: it leaves the open rows
//!    (entering an open Completed section, `ROW_ENTER`), the counts change and the sun travels (`SUN_TRAVEL`,
//!    `SUN`).
//!
//! Reopening a completed task mirrors it: the disc clears (`OVERLAY_OUT`, `EXIT`), the row collapses out of
//! its completed place and is released into the open rows, where it enters. Toggling a row again in the
//! middle reverses it from wherever it is, in place. With reduced motion, movement is instant, fades take
//! at most `REDUCED_FADE`, and the hold keeps its full length (`motion::reduced`).
//!
//! Every duration and curve is `todo_design::motion`'s. Nothing here sleeps or touches GPUI.

use std::collections::BTreeMap;
use std::time::Duration;

use todo_core::ids::TaskId;

use super::RoomKey;
use crate::theme::motion as m;

/// How long things take: the design's durations, or their reduced-motion variants (Windows' *Animation
/// effects* turned off), read when a transition starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pace {
    pub reduced: bool,
}

impl Pace {
    pub const FULL: Self = Self { reduced: false };
    pub const REDUCED: Self = Self { reduced: true };

    /// A movement (a scale, a slide, a stroke drawing itself): instant with reduced motion.
    fn movement(self, ms: u32) -> Duration {
        millis(if self.reduced {
            m::reduced(ms, false)
        } else {
            ms
        })
    }

    /// A pure opacity or colour fade: at most `REDUCED_FADE` with reduced motion.
    fn fade(self, ms: u32) -> Duration {
        millis(if self.reduced {
            m::reduced(ms, true)
        } else {
            ms
        })
    }
}

fn millis(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

/// Linear progress, 0 to 1, of the stretch that starts `start` into the elapsed time `e` and lasts `span`.
/// An instant stretch is complete as soon as it starts.
fn stretch(e: Duration, start: Duration, span: Duration) -> f32 {
    if e < start {
        0.
    } else if span.is_zero() {
        1.
    } else {
        ((e - start).as_secs_f32() / span.as_secs_f32()).min(1.)
    }
}

/// `a` to `b` at `t`, landing exactly on `b` (an overshooting `t` goes past it).
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    if t == 1. { b } else { a + (b - a) * t }
}

/// What a task row looks like at a moment. Every field runs from 0 to 1 except `scale`, which overshoots
/// a little while the disc pops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowLook {
    /// The checkbox disc's opacity: 0 is the empty ring, 1 the filled disc (with its tick).
    pub fill: f32,
    /// The disc's scale.
    pub scale: f32,
    /// How much of the tick is drawn, from the left.
    pub tick: f32,
    /// How much of the title's strike-through is drawn, from the left.
    pub strike: f32,
    /// How far the title has dimmed to `ink-3`.
    pub dim: f32,
    pub opacity: f32,
    /// The row's share of its slot: 1 in place, 0 collapsed (the rows below have closed the gap).
    pub height: f32,
}

impl RowLook {
    /// A row at rest, open or done.
    pub fn rest(done: bool) -> Self {
        let d = f32::from(done);
        Self {
            fill: d,
            scale: 1.,
            tick: d,
            strike: d,
            dim: d,
            opacity: 1.,
            height: 1.,
        }
    }
}

/// How far each row is drawn up its slot, in rows: the share of every slot above it that has collapsed.
/// The view moves a row up by this many row heights, so the rows below a leaving row close its gap.
pub fn lifts(heights: impl IntoIterator<Item = f32>) -> Vec<f32> {
    let mut lift = 0.;
    heights
        .into_iter()
        .map(|h| {
            let this = lift;
            lift += 1. - h;
            this
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Fill, tick and hold in the open rows, then collapse; released into Completed at the end.
    Complete,
    /// The disc clears, then the row collapses out of its completed place; released into the open rows.
    Reopen,
    /// Back to rest where it is, open or done: a toggle that reverses a completion or a reopening.
    Settle { done: bool },
    /// Entering a list at its new place, after a release.
    Enter { done: bool },
}

#[derive(Clone, Copy, Debug)]
struct RowMotion {
    phase: Phase,
    start: Duration,
    from: RowLook,
    pace: Pace,
}

impl RowMotion {
    fn tick_at(&self) -> Duration {
        self.pace.movement(m::CHECK_TICK_DELAY)
    }

    /// When the fill, the tick, the title and any restored height have all landed.
    fn landed(&self) -> Duration {
        let p = self.pace;
        let title = self.tick_at() + p.movement(m::CHECK_TICK).max(p.fade(m::CHECK_TICK));
        title.max(p.movement(m::CHECK_FILL)).max(self.restore_end())
    }

    /// How long putting a partly collapsed or faded row back takes (nothing for a row in place).
    fn restore_end(&self) -> Duration {
        let p = self.pace;
        let mut end = Duration::ZERO;
        if self.from.height < 1. {
            end = end.max(p.movement(m::ROW_ENTER));
        }
        if self.from.opacity < 1. {
            end = end.max(p.fade(m::ROW_ENTER));
        }
        end
    }

    fn clear(&self) -> Duration {
        self.pace.fade(m::OVERLAY_OUT)
    }

    /// When the row starts to leave.
    fn exit_at(&self) -> Duration {
        match self.phase {
            Phase::Complete => self.landed() + millis(m::COMPLETE_HOLD),
            Phase::Reopen => self.clear().max(self.restore_end()),
            Phase::Settle { .. } | Phase::Enter { .. } => Duration::MAX,
        }
    }

    fn exit_span(&self) -> Duration {
        self.pace
            .movement(m::ROW_COLLAPSE)
            .max(self.pace.fade(m::ROW_COLLAPSE))
    }

    fn length(&self) -> Duration {
        let p = self.pace;
        match self.phase {
            Phase::Complete | Phase::Reopen => self.exit_at() + self.exit_span(),
            Phase::Settle { done: true } => self.landed(),
            Phase::Settle { done: false } => self
                .clear()
                .max(p.movement(m::CHECK_TICK))
                .max(p.fade(m::CHECK_TICK))
                .max(self.restore_end()),
            Phase::Enter { .. } => p.movement(m::ROW_ENTER).max(p.fade(m::ROW_ENTER)),
        }
    }

    fn end(&self) -> Duration {
        self.start + self.length()
    }

    fn look(&self, now: Duration) -> RowLook {
        let e = now.saturating_sub(self.start);
        let (p, f) = (self.pace, self.from);
        let zero = Duration::ZERO;
        // Filling: the disc pops in from its starting scale (or from where it is), and the tick and the
        // title follow after the tick delay.
        let filling = || {
            let t = stretch(e, zero, p.movement(m::CHECK_FILL));
            let from_scale = if f.fill > 0. {
                f.scale
            } else {
                m::CHECK_FILL_FROM_SCALE
            };
            let draw = m::STANDARD.ease(stretch(e, self.tick_at(), p.movement(m::CHECK_TICK)));
            let dim = m::STANDARD.ease(stretch(e, self.tick_at(), p.fade(m::CHECK_TICK)));
            RowLook {
                fill: mix(f.fill, 1., m::POP.ease(t).min(1.)),
                scale: mix(from_scale, 1., m::POP.ease(t)),
                tick: mix(f.tick, 1., draw),
                strike: mix(f.strike, 1., draw),
                dim: mix(f.dim, 1., dim),
                ..f
            }
        };
        // Clearing: the disc fades out with its tick, and the title loses its strike and brightens.
        let clearing = || {
            let t = stretch(e, zero, self.clear());
            let undraw = m::STANDARD.ease(stretch(e, zero, p.movement(m::CHECK_TICK)));
            let undim = m::STANDARD.ease(stretch(e, zero, p.fade(m::CHECK_TICK)));
            RowLook {
                fill: mix(f.fill, 0., m::EXIT.ease(t)),
                scale: mix(f.scale, 1., t),
                tick: if t >= 1. { 0. } else { f.tick },
                strike: mix(f.strike, 0., undraw),
                dim: mix(f.dim, 0., undim),
                ..f
            }
        };
        // A row that was partly collapsed or faded comes back first; a leaving one goes.
        let placed = |look: RowLook| {
            let back_h = m::ENTER.ease(stretch(e, zero, p.movement(m::ROW_ENTER)));
            let back_o = m::ENTER.ease(stretch(e, zero, p.fade(m::ROW_ENTER)));
            let at = self.exit_at();
            // With reduced motion the row only fades, and leaves the layout at once when it's released.
            let gone_h = if p.reduced {
                0.
            } else {
                m::EXIT.ease(stretch(e, at, p.movement(m::ROW_COLLAPSE)))
            };
            let gone_o = m::EXIT.ease(stretch(e, at, p.fade(m::ROW_COLLAPSE)));
            RowLook {
                height: mix(f.height, 1., back_h) * (1. - gone_h),
                opacity: mix(f.opacity, 1., back_o) * (1. - gone_o),
                ..look
            }
        };
        match self.phase {
            Phase::Complete | Phase::Settle { done: true } => placed(filling()),
            Phase::Reopen | Phase::Settle { done: false } => placed(clearing()),
            Phase::Enter { done } => RowLook {
                height: m::ENTER.ease(stretch(e, zero, p.movement(m::ROW_ENTER))),
                opacity: m::ENTER.ease(stretch(e, zero, p.fade(m::ROW_ENTER))),
                ..RowLook::rest(done)
            },
        }
    }

    fn wake(&self, now: Duration) -> Wake {
        if self.phase == Phase::Complete {
            // Holding still: nothing to draw until the row starts to leave.
            let (quiet, exit) = (self.start + self.landed(), self.start + self.exit_at());
            if now >= quiet && now < exit {
                return Wake::At(exit);
            }
        }
        Wake::Frame
    }
}

/// When the view needs its next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// At rest: no frames, no timers.
    Idle,
    /// Nothing moves until then (a row holding still).
    At(Duration),
    /// Something moves: the next frame.
    Frame,
}

impl Wake {
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Frame, _) | (_, Self::Frame) => Self::Frame,
            (Self::Idle, w) | (w, Self::Idle) => w,
            (Self::At(a), Self::At(b)) => Self::At(a.min(b)),
        }
    }
}

/// A row that finished leaving: what the list holds changes now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// A completed task left the open rows (and enters Completed, if that's open).
    Completed(TaskId),
    /// A reopened task left its completed place (and enters the open rows).
    Reopened(TaskId),
}

/// Where a toggled task stays while its choreography plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// In the open rows, as if still open (`UiState::holding`).
    Holding,
    /// In its completed place, as if still done (`UiState::reopening`).
    Reopening,
    /// Where it already belongs: the toggle reversed a choreography that hadn't released it.
    InPlace,
}

/// The sun's place on its arc, gliding from where it was to where the count puts it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Travel {
    room: RoomKey,
    from: f32,
    to: f32,
    start: Duration,
    span: Duration,
}

impl Travel {
    fn at(&self, now: Duration) -> f32 {
        let t = stretch(now, self.start, self.span);
        mix(self.from, self.to, m::SUN.ease(t))
    }

    fn moving(&self, now: Duration) -> bool {
        self.from != self.to && now < self.start + self.span
    }
}

/// The window's motion: rows in their choreography and the sun.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    rows: BTreeMap<TaskId, RowMotion>,
    sun: Option<Travel>,
}

impl Motion {
    /// How the row of task `id` looks now, or `None` when it's at rest.
    pub fn row(&self, id: TaskId, now: Duration) -> Option<RowLook> {
        self.rows.get(&id).map(|r| r.look(now))
    }

    fn start(&mut self, id: TaskId, phase: Phase, now: Duration, pace: Pace, rest: bool) {
        let from = self.row(id, now).unwrap_or_else(|| RowLook::rest(rest));
        self.rows.insert(
            id,
            RowMotion {
                phase,
                start: now,
                from,
                pace,
            },
        );
    }

    /// Task `id` was just completed. A task that was being reopened settles back where it was; any other
    /// starts the completion in the open rows.
    pub fn complete(&mut self, id: TaskId, now: Duration, pace: Pace) -> Keep {
        if self.phase(id) == Some(Phase::Reopen) {
            self.start(id, Phase::Settle { done: true }, now, pace, true);
            Keep::InPlace
        } else {
            self.start(id, Phase::Complete, now, pace, false);
            Keep::Holding
        }
    }

    /// Task `id` was just reopened. A task that was being completed settles back where it was; any other
    /// starts leaving its completed place.
    pub fn reopen(&mut self, id: TaskId, now: Duration, pace: Pace) -> Keep {
        if self.phase(id) == Some(Phase::Complete) {
            self.start(id, Phase::Settle { done: false }, now, pace, false);
            Keep::InPlace
        } else {
            self.start(id, Phase::Reopen, now, pace, true);
            Keep::Reopening
        }
    }

    fn phase(&self, id: TaskId) -> Option<Phase> {
        self.rows.get(&id).map(|r| r.phase)
    }

    /// Tasks completing in the open rows (`UiState::holding`).
    pub fn holding(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.ids(Phase::Complete)
    }

    /// Tasks leaving their completed place (`UiState::reopening`).
    pub fn reopening(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.ids(Phase::Reopen)
    }

    fn ids(&self, phase: Phase) -> impl Iterator<Item = TaskId> + '_ {
        self.rows
            .iter()
            .filter(move |(_, r)| r.phase == phase)
            .map(|(id, _)| *id)
    }

    /// Follows the data when it changed under a choreography (another device, an agent, a refused
    /// command): `done` says whether each task is done now, or `None` when it's gone. A completion whose
    /// task is open again, or a reopening whose task is done again, settles back in place; a row at
    /// rest or entering that no longer matches its task's state stops at once.
    pub fn reconcile(&mut self, now: Duration, pace: Pace, done: impl Fn(TaskId) -> Option<bool>) {
        self.rows.retain(|&id, r| match (done(id), r.phase) {
            (None, _) => false,
            (Some(false), Phase::Complete) | (Some(true), Phase::Reopen) => {
                let done = r.phase == Phase::Reopen;
                *r = RowMotion {
                    phase: Phase::Settle { done },
                    start: now,
                    from: r.look(now),
                    pace,
                };
                true
            }
            (Some(d), Phase::Settle { done } | Phase::Enter { done }) => d == done,
            (Some(_), _) => true,
        });
    }

    /// Moves every row past its end: rows that finished leaving are released (and enter their new place,
    /// timed from when they left), finished ones come to rest.
    pub fn advance(&mut self, now: Duration) -> Vec<Release> {
        let mut released = Vec::new();
        self.rows.retain(|&id, r| {
            if now < r.end() {
                return true;
            }
            let (release, done) = match r.phase {
                Phase::Complete => (Release::Completed(id), true),
                Phase::Reopen => (Release::Reopened(id), false),
                Phase::Settle { .. } | Phase::Enter { .. } => return false,
            };
            released.push(release);
            *r = RowMotion {
                phase: Phase::Enter { done },
                start: r.end(),
                from: RowLook {
                    height: 0.,
                    opacity: 0.,
                    ..RowLook::rest(done)
                },
                pace: r.pace,
            };
            now < r.end()
        });
        released
    }

    /// Stops a row's motion where it is (a released row that has no place in the list to enter).
    pub fn forget(&mut self, id: TaskId) {
        self.rows.remove(&id);
    }

    /// Rows entering after a release, which the view checks against the rows it shows.
    pub fn entering(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.rows
            .iter()
            .filter(|(_, r)| matches!(r.phase, Phase::Enter { .. }))
            .map(|(id, _)| *id)
    }

    /// Everything at rest at once, as on switching rooms: completions and reopenings are released
    /// without playing on.
    pub fn settle_all(&mut self) {
        self.rows.clear();
    }

    /// Points the sun at `target` (its place on the arc, or `None` when the room has no sun). In the same
    /// room it glides there from wherever it is now; a new room, or a sun that appears or goes, snaps.
    pub fn aim_sun(&mut self, room: RoomKey, target: Option<f32>, now: Duration, pace: Pace) {
        let Some(to) = target else {
            self.sun = None;
            return;
        };
        match self.sun {
            Some(s) if s.room == room && s.to == to => {}
            Some(s) if s.room == room => {
                self.sun = Some(Travel {
                    room,
                    from: s.at(now),
                    to,
                    start: now,
                    span: pace.movement(m::SUN_TRAVEL),
                });
            }
            _ => {
                self.sun = Some(Travel {
                    room,
                    from: to,
                    to,
                    start: now,
                    span: Duration::ZERO,
                });
            }
        }
    }

    /// The sun's place on its arc now (0 to 1), or `None` without a sun.
    pub fn sun(&self, now: Duration) -> Option<f32> {
        self.sun.map(|s| s.at(now))
    }

    /// When the view needs its next frame. At rest it's [`Wake::Idle`]: no frames and no timers.
    pub fn wake(&self, now: Duration) -> Wake {
        let sun = if self.sun.is_some_and(|s| s.moving(now)) {
            Wake::Frame
        } else {
            Wake::Idle
        };
        self.rows.values().fold(sun, |w, r| w.or(r.wake(now)))
    }

    /// Whether a row has reached its end by `now`, so [`Motion::advance`] has something to do.
    pub fn due(&self, now: Duration) -> bool {
        self.rows.values().any(|r| now >= r.end())
    }
}

#[cfg(test)]
mod tests;
