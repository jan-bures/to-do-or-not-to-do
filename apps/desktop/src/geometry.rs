//! Geometry of the header landscape: the sun's arc, where the sun sits for N of M done, and the two hills.
//!
//! Pure functions in local coordinates (origin at the strip's top-left, logical pixels), so they can be
//! tested without a window. `ui::landscape` turns them into GPUI paths and caches those per size.

use std::f32::consts::PI;

/// A point in the strip's local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

/// One cubic bézier segment, from the previous point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    pub c1: Pt,
    pub c2: Pt,
    pub to: Pt,
}

/// Inset of the arc's ends from the strip's sides.
const ARC_INSET: f32 = 28.0;
/// Distance of the arc's ends above the strip's bottom.
const ARC_BASE_FROM_BOTTOM: f32 = 13.0;
/// Height of the arc's apex above the strip's top.
const ARC_PEAK: f32 = 10.0;

/// How far along the day is: the fraction of tasks done, clamped to `0..=1`. Zero tasks is zero progress.
pub fn progress(done: u32, total: u32) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (done.min(total) as f32) / (total as f32)
}

/// The point at fraction `t` (0..=1) along the sun's arc in a `w` × `h` strip: a half sine from the left
/// end to the right end, peaking in the middle.
pub fn arc_point(t: f32, w: f32, h: f32) -> Pt {
    let t = t.clamp(0.0, 1.0);
    let (x0, x1) = (ARC_INSET, (w - ARC_INSET).max(ARC_INSET));
    let base = h - ARC_BASE_FROM_BOTTOM;
    let peak = ARC_PEAK.min(base);
    pt(x0 + (x1 - x0) * t, base - (base - peak) * (PI * t).sin())
}

/// Where the sun sits for `done` of `total` tasks.
pub fn sun_position(done: u32, total: u32, w: f32, h: f32) -> Pt {
    arc_point(progress(done, total), w, h)
}

/// A polyline along the arc from fraction `t0` to `t1`, with `segments` straight pieces.
pub fn arc_polyline(t0: f32, t1: f32, segments: usize, w: f32, h: f32) -> Vec<Pt> {
    let segments = segments.max(1);
    (0..=segments)
        .map(|i| arc_point(t0 + (t1 - t0) * i as f32 / segments as f32, w, h))
        .collect()
}

/// The arc split at the sun: (travelled, remaining). The travelled part is empty at zero progress and the
/// remaining part is empty when everything is done. Segment counts follow the mockup (48 + 64 over the arc).
pub fn split_arc(done: u32, total: u32, w: f32, h: f32) -> (Vec<Pt>, Vec<Pt>) {
    let t = progress(done, total);
    let travelled = if t > 0.0 {
        arc_polyline(0.0, t, ((48.0 * t).ceil() as usize).max(2), w, h)
    } else {
        Vec::new()
    };
    let remaining = if t < 1.0 {
        arc_polyline(t, 1.0, ((64.0 * (1.0 - t)).ceil() as usize).max(2), w, h)
    } else {
        Vec::new()
    };
    (travelled, remaining)
}

/// A closed hill: starts at `start` on the left edge, follows `curves` to the right edge, then closes along
/// the strip's bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Hill {
    pub start: Pt,
    pub curves: Vec<Cubic>,
}

/// Expands an SVG `C` followed by `S` segments into explicit cubics (`S` mirrors the previous second control
/// point about the current point).
fn smooth_chain(first: (Pt, Pt, Pt), smooth: &[(Pt, Pt)]) -> Vec<Cubic> {
    let mut out = vec![Cubic {
        c1: first.0,
        c2: first.1,
        to: first.2,
    }];
    for &(c2, to) in smooth {
        let prev = out[out.len() - 1];
        let c1 = pt(2.0 * prev.to.x - prev.c2.x, 2.0 * prev.to.y - prev.c2.y);
        out.push(Cubic { c1, c2, to });
    }
    out
}

/// The far and near hills of the slim (48 px) strip, as drawn in the F2 mockups.
pub fn hills(w: f32, h: f32) -> (Hill, Hill) {
    let x = |f: f32| w * f;
    let f0 = h - 15.0;
    let n0 = h - 7.0;
    let far = Hill {
        start: pt(0.0, f0),
        curves: smooth_chain(
            (
                pt(x(0.12), f0 - 6.0),
                pt(x(0.22), f0 + 3.0),
                pt(x(0.36), f0 - 2.0),
            ),
            &[
                (pt(x(0.62), f0 - 8.0), pt(x(0.74), f0 - 1.0)),
                (pt(x(0.93), f0 + 2.0), pt(w, f0 - 4.0)),
            ],
        ),
    };
    let near = Hill {
        start: pt(0.0, n0),
        curves: smooth_chain(
            (
                pt(x(0.15), n0 - 5.0),
                pt(x(0.30), n0 + 3.0),
                pt(x(0.48), n0 - 1.0),
            ),
            &[
                (pt(x(0.76), n0 - 5.0), pt(x(0.90), n0)),
                (pt(x(0.98), n0 + 1.0), pt(w, n0 - 2.0)),
            ],
        ),
    };
    (far, near)
}

/// Stars of the dark sky: (x fraction, y fraction, radius).
pub const STARS: [(f32, f32, f32); 10] = [
    (0.06, 0.18, 1.1),
    (0.15, 0.42, 0.8),
    (0.24, 0.12, 1.0),
    (0.37, 0.30, 0.8),
    (0.47, 0.10, 1.2),
    (0.58, 0.36, 0.8),
    (0.66, 0.16, 1.0),
    (0.79, 0.28, 0.9),
    (0.88, 0.10, 1.1),
    (0.95, 0.40, 0.8),
];

/// Top of the dark horizon band, which sits behind the hills.
pub fn horizon_top(h: f32) -> f32 {
    h - ARC_BASE_FROM_BOTTOM - 12.0
}

/// The sun's parts, relative to its centre.
pub mod sun {
    /// Radius of the disc.
    pub const DISC: f32 = 6.2;
    /// Radius of the soft halo behind it.
    pub const HALO: f32 = 11.0;
    /// Rays run from this radius outwards...
    pub const RAY_OUTER: f32 = 11.5;
    /// ...for this length.
    pub const RAY_LEN: f32 = 3.0;
    pub const RAY_WIDTH: f32 = 1.6;
    pub const RAYS: usize = 8;
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 800.0;
    const H: f32 = 48.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn progress_is_clamped_and_safe_for_empty_lists() {
        assert_eq!(progress(0, 0), 0.0);
        assert_eq!(progress(5, 0), 0.0);
        assert_eq!(progress(3, 12), 0.25);
        assert_eq!(progress(12, 12), 1.0);
        assert_eq!(progress(20, 12), 1.0);
    }

    #[test]
    fn the_sun_rises_from_the_left_end_and_sets_at_the_right_end() {
        let start = sun_position(0, 12, W, H);
        assert!(close(start.x, 28.0) && close(start.y, 35.0), "{start:?}");
        let end = sun_position(12, 12, W, H);
        assert!(close(end.x, 772.0) && close(end.y, 35.0), "{end:?}");
    }

    #[test]
    fn half_done_puts_the_sun_at_the_apex() {
        let mid = sun_position(6, 12, W, H);
        assert!(close(mid.x, 400.0) && close(mid.y, 10.0), "{mid:?}");
    }

    #[test]
    fn three_of_twelve_matches_the_mockup() {
        // F2 desktop-light: 3 of 12 done, the sun a quarter of the way along, well up the arc.
        let p = sun_position(3, 12, W, H);
        assert!(close(p.x, 214.0), "{p:?}");
        let expected_y = 35.0 - 25.0 * (PI / 4.0).sin();
        assert!(close(p.y, expected_y), "{p:?}");
    }

    #[test]
    fn the_sun_moves_right_and_is_symmetric() {
        let total = 9;
        let xs: Vec<f32> = (0..=total)
            .map(|d| sun_position(d, total, W, H).x)
            .collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]));
        for d in 0..=total {
            let a = sun_position(d, total, W, H);
            let b = sun_position(total - d, total, W, H);
            assert!(close(a.y, b.y));
            assert!(close(a.x - 28.0, 772.0 - b.x));
        }
    }

    #[test]
    fn the_arc_is_split_exactly_at_the_sun() {
        let (travelled, remaining) = split_arc(7, 22, W, H);
        let sun = sun_position(7, 22, W, H);
        assert_eq!(travelled.first().copied(), Some(arc_point(0.0, W, H)));
        assert_eq!(travelled.last().copied(), Some(sun));
        assert_eq!(remaining.first().copied(), Some(sun));
        assert_eq!(remaining.last().copied(), Some(arc_point(1.0, W, H)));
        let (t0, r0) = split_arc(0, 5, W, H);
        assert!(t0.is_empty() && r0.len() > 2);
        let (t1, r1) = split_arc(5, 5, W, H);
        assert!(r1.is_empty() && t1.len() > 2);
    }

    #[test]
    fn the_arc_follows_the_window_width() {
        let narrow = sun_position(1, 2, 480.0, H);
        assert!(close(narrow.x, 240.0));
        // Degenerate widths don't produce an inverted arc.
        let tiny = sun_position(1, 2, 10.0, H);
        assert!(tiny.x >= 28.0 - 1e-3);
    }

    #[test]
    fn hills_span_the_strip_and_stay_inside_it() {
        let (far, near) = hills(W, H);
        for hill in [&far, &near] {
            assert_eq!(hill.start.x, 0.0);
            assert_eq!(hill.curves.last().map(|c| c.to.x), Some(W));
            for c in &hill.curves {
                for p in [c.c1, c.c2, c.to] {
                    assert!(p.y > 0.0 && p.y < H, "{p:?}");
                }
            }
        }
        // The near hill sits lower (in front of) the far one.
        assert!(near.start.y > far.start.y);
    }

    #[test]
    fn smooth_segments_mirror_the_previous_control_point() {
        let (far, _) = hills(W, H);
        let (a, b) = (far.curves[0], far.curves[1]);
        assert!(close(b.c1.x - a.to.x, a.to.x - a.c2.x));
        assert!(close(b.c1.y - a.to.y, a.to.y - a.c2.y));
    }
}
