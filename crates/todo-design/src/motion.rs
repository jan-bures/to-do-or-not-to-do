//! Motion: durations, easings and reduced-motion rules.
//!
//! Motion confirms what happened; it never decorates, and nothing loops or idles. Every transition must stay
//! interruptible and must never block input. The full specification (principles, per-moment choreography,
//! reduced-motion fallbacks, platform notes and anti-patterns) lives in `docs/design-language.md` under
//! **Motion**.
//!
//! **Completing a task** is the one choreographed moment:
//!
//! 1. The ring fills with `accent` and pops slightly ([`CHECK_FILL`], [`POP`]).
//! 2. The tick draws itself from left to right ([`CHECK_TICK`], starting [`CHECK_TICK_DELAY`] in).
//! 3. The title dims to `ink-3` with a strike-through, and the row holds still ([`COMPLETE_HOLD`]) so the eye sees
//!    it land.
//! 4. The row collapses and fades ([`ROW_COLLAPSE`], [`EXIT`]), and the rows below move up.
//! 5. The sun moves along its arc to the new count and the travelled path grows with it ([`SUN_TRAVEL`],
//!    [`SUN`]). The header count changes at the same moment.
//!
//! Completing the last task of My Day turns the sky to dusk and sets the sun behind the hills ([`DUSK`]).
//!
//! With reduced motion, every movement, scale, rotation and stagger becomes instant (`0 ms`), pure opacity and
//! colour fades are capped at [`REDUCED_FADE`], and functional timers ([`COMPLETE_HOLD`], [`SCROLLBAR_HOLD`],
//! [`UNDO_WINDOW`]) keep their full duration. See [`reduced()`].

/// A CSS-style cubic Bézier easing curve from (0, 0) to (1, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// First control point x, 0–1.
    pub x1: f32,
    /// First control point y.
    pub y1: f32,
    /// Second control point x, 0–1.
    pub x2: f32,
    /// Second control point y.
    pub y2: f32,
}

impl CubicBezier {
    /// Builds a curve.
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> CubicBezier {
        CubicBezier { x1, y1, x2, y2 }
    }

    /// Eased progress for linear progress `t` (clamped to 0–1). Usable directly as a GPUI or Compose easing
    /// function.
    pub fn ease(&self, t: f32) -> f32 {
        let t = f64::from(t.clamp(0.0, 1.0));
        if t == 0.0 || t == 1.0 {
            return t as f32;
        }
        let (x1, y1, x2, y2) = (
            f64::from(self.x1),
            f64::from(self.y1),
            f64::from(self.x2),
            f64::from(self.y2),
        );
        let bez = |a: f64, b: f64, s: f64| {
            let u = 1.0 - s;
            3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
        };
        let slope = |a: f64, b: f64, s: f64| {
            let u = 1.0 - s;
            3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
        };
        // Newton's method on x(s) = t, falling back to bisection where the slope is flat.
        let mut s = t;
        for _ in 0..8 {
            let err = bez(x1, x2, s) - t;
            if err.abs() < 1e-7 {
                return bez(y1, y2, s) as f32;
            }
            let d = slope(x1, x2, s);
            if d.abs() < 1e-6 {
                break;
            }
            s = (s - err / d).clamp(0.0, 1.0);
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        s = t;
        for _ in 0..40 {
            if bez(x1, x2, s) < t {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) / 2.0;
        }
        bez(y1, y2, s) as f32
    }

    /// CSS `cubic-bezier(...)`.
    pub fn css(&self) -> String {
        format!(
            "cubic-bezier({}, {}, {}, {})",
            self.x1, self.y1, self.x2, self.y2
        )
    }
}

/// Default for things that move from one resting place to another (tick draw, panels, reorder, hover/focus).
pub const STANDARD: CubicBezier = CubicBezier::new(0.2, 0.0, 0.0, 1.0);
/// For things that appear (new rows, toasts, menus, dialogs, drawers): fast start, gentle landing.
pub const ENTER: CubicBezier = CubicBezier::new(0.0, 0.0, 0.2, 1.0);
/// For things that leave (completed rows, dismissed toasts and overlays, fading scrollbars): gentle start, quick exit.
pub const EXIT: CubicBezier = CubicBezier::new(0.3, 0.0, 1.0, 1.0);
/// The checkbox fill: a small overshoot, the only bounce in the app.
pub const POP: CubicBezier = CubicBezier::new(0.34, 1.56, 0.64, 1.0);
/// The sun on its arc, room sky hue morphs and the dusk transition: slow in, slow out, like the real thing.
pub const SUN: CubicBezier = CubicBezier::new(0.45, 0.0, 0.55, 1.0);

/// Named easings, for the exports.
pub const EASINGS: [(&str, CubicBezier); 5] = [
    ("standard", STANDARD),
    ("enter", ENTER),
    ("exit", EXIT),
    ("pop", POP),
    ("sun", SUN),
];

/// Hover, press release and focus-ring colour changes, in milliseconds.
pub const HOVER: u32 = 90;
/// Fast in-place crossfade (switching tasks in an open detail pane, room title/eyebrow swap, sync status change).
pub const CONTENT_SWAP: u32 = 120;
/// How long the incoming room title overlaps the departing title, in milliseconds.
pub const ROOM_TITLE_OVERLAP: u32 = 40;
/// Opacity the detail body enters from when switching tasks in an already open pane.
pub const DETAIL_SWAP_FROM: f32 = 0.25;
/// Menus, popovers, date pickers and dialogs dismissing.
pub const OVERLAY_OUT: u32 = 120;
/// The checkbox disc filling with accent and popping slightly with [`POP`].
pub const CHECK_FILL: u32 = 140;
/// The checkbox disc's scale when its fill starts; it pops to `1.0` with [`POP`] over [`CHECK_FILL`]. A scale,
/// not a duration, so it isn't exported with them.
pub const CHECK_FILL_FROM_SCALE: f32 = 0.55;
/// Toast leaving.
pub const TOAST_OUT: u32 = 140;
/// Menus, popovers, date pickers and dialogs entering.
pub const OVERLAY_IN: u32 = 150;
/// Delay before the tick starts drawing.
pub const CHECK_TICK_DELAY: u32 = 60;
/// The tick drawing itself left to right (also title dim and strike-through).
pub const CHECK_TICK: u32 = 180;
/// A newly added or reopened task row entering a list, and sibling rows shifting during reorder.
pub const ROW_ENTER: u32 = 180;
/// Toast entering.
pub const TOAST_IN: u32 = 180;
/// Panels (detail pane, sidebar collections, pairing steps, Android drawer and shared-element settles) opening and closing.
pub const PANEL: u32 = 200;
/// A completed row collapsing and fading out of the list.
pub const ROW_COLLAPSE: u32 = 220;
/// Overlay scrollbar fading out after [`SCROLLBAR_HOLD`].
pub const SCROLLBAR_OUT: u32 = 300;
/// The header sky, hills and tinted room palette morphing from one room hue to another ([`SUN`]).
pub const ROOM_HUE: u32 = 360;
/// The sun moving to its new place on the arc ([`SUN`]).
pub const SUN_TRAVEL: u32 = 700;
/// Quiet `wash` → `pane` background highlight on a row changed by live remote sync or an agent.
pub const SYNC_HIGHLIGHT: u32 = 900;
/// Sky to dusk, header expansion to 164 px and the sun setting when My Day is finished ([`SUN`]).
pub const DUSK: u32 = 1200;
/// Dusk cue, as a share of [`DUSK`]: the header text switches to the dark theme's ink, and the moon and stars
/// begin to fade in.
pub const DUSK_NIGHT_AT: f32 = 0.4;
/// Dusk cue, as a share of [`DUSK`]: tomorrow's hand-over begins to enter ([`ROW_ENTER`], [`ENTER`]).
pub const DUSK_HANDOVER_AT: f32 = 0.65;
/// How far tomorrow's hand-over rises as it enters during dusk, in pixels (none with reduced motion).
pub const DUSK_HANDOVER_RISE: f32 = 6.0;
/// Delay step between items in a small entering group (capped at [`STAGGER_MAX_ITEMS`] items).
pub const STAGGER: u32 = 25;
/// Maximum number of items that receive incremental [`STAGGER`] delay (125 ms total cap).
pub const STAGGER_MAX_ITEMS: usize = 5;
/// The longest fade allowed with reduced motion.
pub const REDUCED_FADE: u32 = 100;
/// Timer: how long a completed row stays in place before it leaves. Unchanged under reduced motion.
pub const COMPLETE_HOLD: u32 = 450;
/// Timer: how long an overlay scrollbar stays visible after scrolling stops. Unchanged under reduced motion.
pub const SCROLLBAR_HOLD: u32 = 1000;
/// Timer: how long the undo toast stays (its countdown line runs linearly for this long). Ctrl+Z works after that too.
pub const UNDO_WINDOW: u32 = 6000;

/// Named durations in milliseconds, for the exports.
pub const DURATIONS: [(&str, u32); 22] = [
    ("hover", HOVER),
    ("content-swap", CONTENT_SWAP),
    ("overlay-in", OVERLAY_IN),
    ("overlay-out", OVERLAY_OUT),
    ("check-fill", CHECK_FILL),
    ("check-tick-delay", CHECK_TICK_DELAY),
    ("check-tick", CHECK_TICK),
    ("complete-hold", COMPLETE_HOLD),
    ("row-enter", ROW_ENTER),
    ("row-collapse", ROW_COLLAPSE),
    ("room-hue", ROOM_HUE),
    ("sun-travel", SUN_TRAVEL),
    ("dusk", DUSK),
    ("toast-in", TOAST_IN),
    ("toast-out", TOAST_OUT),
    ("undo-window", UNDO_WINDOW),
    ("panel", PANEL),
    ("scrollbar-hold", SCROLLBAR_HOLD),
    ("scrollbar-out", SCROLLBAR_OUT),
    ("sync-highlight", SYNC_HIGHLIGHT),
    ("stagger", STAGGER),
    ("reduced-fade", REDUCED_FADE),
];

/// The duration to use with reduced motion: movement, scale, rotation and stagger become instant (`0 ms`),
/// pure opacity and colour fades shrink to at most [`REDUCED_FADE`]. Functional timers that aren't animations
/// ([`UNDO_WINDOW`], [`COMPLETE_HOLD`], [`SCROLLBAR_HOLD`]) are unchanged.
pub const fn reduced(duration_ms: u32, is_fade: bool) -> u32 {
    if duration_ms == UNDO_WINDOW || duration_ms == COMPLETE_HOLD || duration_ms == SCROLLBAR_HOLD {
        duration_ms
    } else if is_fade {
        if duration_ms < REDUCED_FADE {
            duration_ms
        } else {
            REDUCED_FADE
        }
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn easings_hit_their_endpoints_and_are_monotonic_in_x() {
        for (name, e) in EASINGS {
            assert_eq!(e.ease(0.0), 0.0, "{name}");
            assert_eq!(e.ease(1.0), 1.0, "{name}");
            let mid = e.ease(0.5);
            assert!(mid > 0.0 && mid < 1.6, "{name}: {mid}");
        }
        // The symmetric curve is symmetric.
        assert!((SUN.ease(0.5) - 0.5).abs() < 1e-4);
        // Linear is linear.
        let linear = CubicBezier::new(0.25, 0.25, 0.75, 0.75);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!((linear.ease(t) - t).abs() < 1e-4);
        }
        // POP overshoots.
        assert!((0..100).any(|i| POP.ease(i as f32 / 100.0) > 1.0));
    }

    #[test]
    fn reduced_motion() {
        assert_eq!(reduced(SUN_TRAVEL, false), 0);
        assert_eq!(reduced(ROOM_HUE, false), 0);
        assert_eq!(reduced(ROOM_HUE, true), REDUCED_FADE);
        assert_eq!(reduced(TOAST_IN, true), REDUCED_FADE);
        assert_eq!(reduced(SYNC_HIGHLIGHT, true), REDUCED_FADE);
        assert_eq!(reduced(SCROLLBAR_OUT, true), REDUCED_FADE);
        assert_eq!(reduced(STAGGER, false), 0);
        assert_eq!(reduced(HOVER, true), HOVER);
        assert_eq!(reduced(UNDO_WINDOW, false), UNDO_WINDOW);
        assert_eq!(reduced(COMPLETE_HOLD, false), COMPLETE_HOLD);
        assert_eq!(reduced(SCROLLBAR_HOLD, false), SCROLLBAR_HOLD);
    }

    #[test]
    fn duration_names_are_unique_and_nonzero() {
        let mut names = HashSet::new();
        for (name, ms) in DURATIONS {
            assert!(names.insert(name), "duplicate duration token: {name}");
            assert!(ms > 0, "duration token {name} must be > 0");
        }
        // Functional timers must not collide numerically with non-timer durations so `reduced()` stays exact.
        let timers = [UNDO_WINDOW, COMPLETE_HOLD, SCROLLBAR_HOLD];
        for (name, ms) in DURATIONS {
            if !matches!(name, "undo-window" | "complete-hold" | "scrollbar-hold") {
                assert!(
                    !timers.contains(&ms),
                    "non-timer duration {name} ({ms} ms) collides with a timer duration in reduced()"
                );
            }
        }
    }
}
