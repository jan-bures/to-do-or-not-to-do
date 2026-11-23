//! The taller phone panorama. Legacy desktop/web scene geometry remains in the parent module.

use super::{PathEl, Point, STARS, Star, pt};
use crate::layout::phone::{HEADER_SCENE_COVER, HEADER_SCENE_INNER};
use std::f32::consts::PI;

pub(super) const FAR_POINTS: [(f32, f32); 9] = [
    (0.12, -6.0),
    (0.22, 3.0),
    (0.36, -2.0),
    (0.50, -7.0),
    (0.62, -8.0),
    (0.74, -1.0),
    (0.86, 6.0),
    (0.93, 2.0),
    (1.00, -4.0),
];
pub(super) const NEAR_POINTS: [(f32, f32); 9] = [
    (0.15, -5.0),
    (0.30, 3.0),
    (0.48, -1.0),
    (0.66, -5.0),
    (0.76, -5.0),
    (0.90, 0.0),
    (1.04, 5.0),
    (0.98, 1.0),
    (1.00, -2.0),
];
const RIDGE_POINTS: [(f32, f32); 9] = [
    (0.14, -5.0),
    (0.28, -9.0),
    (0.42, -3.0),
    (0.56, 3.0),
    (0.70, -7.0),
    (0.84, -4.0),
    (0.92, -2.0),
    (0.97, -5.0),
    (1.00, -3.0),
];

/// A hill's baseline above the bottom of the canvas and Bézier amplitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hill {
    /// Baseline above the bottom, in dp.
    pub depth: f32,
    /// Multiplier for the control point offsets.
    pub amplitude: f32,
}

/// Dimensions of a phone arc at a scene endpoint. Sampled as a half sine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    /// Horizontal inset (dp).
    pub pad: f32,
    /// Start and end's distance above the bottom (dp).
    pub base_depth: f32,
    /// Middle's distance from the top (dp).
    pub peak: f32,
}

impl Arc {
    /// Sample the scene's arc at progress `t`, clamped to 0–1.
    #[must_use]
    pub fn point(self, width: f32, height: f32, t: f32) -> Point {
        let t = t.clamp(0.0, 1.0);
        let pad = self.pad.min(width * 0.25);
        let base = height - self.base_depth;
        pt(
            pad + (width - 2.0 * pad) * t,
            base - (base - self.peak) * (PI * t).sin(),
        )
    }
}

/// The arc and three hills, from back to front, at a phone panorama endpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panorama {
    /// Three hills (ridge, far, near).
    pub hills: [Hill; 3],
    /// The sun's arc.
    pub arc: Arc,
}

/// Rest geometry, selected by posture; horizontal inset fits the pane's actual width.
#[must_use]
pub fn rest(width: f32, inner: bool) -> Panorama {
    let pad = if width > 420.0 { 40.0 } else { 36.0 };
    if inner {
        panorama([(38.0, 1.5), (27.0, 1.6), (14.0, 1.4)], pad, 34.0, 12.0)
    } else {
        panorama([(34.0, 1.5), (24.0, 1.6), (12.0, 1.4)], pad, 30.0, 12.0)
    }
}

/// Expanded cover geometry at full pull-down reach.
#[must_use]
pub fn reach(width: f32) -> Panorama {
    panorama(
        [(84.0, 2.8), (58.0, 2.6), (28.0, 2.2)],
        rest(width, false).arc.pad,
        60.0,
        44.0,
    )
}

/// Finished My Day keeps the setting sun behind the hills, including during reach.
#[must_use]
pub fn dusk(reached: bool) -> Panorama {
    let (layers, low, peak) = if reached {
        ([(92.0, 3.2), (70.0, 3.5), (34.0, 3.4)], 76.0, 70.0)
    } else {
        ([(74.0, 2.6), (58.0, 3.0), (26.0, 3.0)], 64.0, 46.0)
    };
    panorama(layers, 56.0, low, peak)
}

fn panorama(layers: [(f32, f32); 3], pad: f32, base_depth: f32, peak: f32) -> Panorama {
    Panorama {
        hills: layers.map(|(depth, amplitude)| Hill { depth, amplitude }),
        arc: Arc {
            pad,
            base_depth,
            peak,
        },
    }
}

/// Unit Bézier coordinates: x is a fraction of width, y is an amplitude offset from the baseline.
/// Clients can transform these and rewind their paths in place while the scene grows.
#[must_use]
pub fn hill_controls() -> [[Point; 9]; 3] {
    [RIDGE_POINTS, FAR_POINTS, NEAR_POINTS].map(|points| points.map(|(x, y)| pt(x, y)))
}

/// Landscape stars (the legacy constellation), in unit coordinates.
#[must_use]
pub fn stars() -> [Star; 10] {
    STARS.map(|(x, y, radius)| Star {
        center: pt(x, y),
        radius,
    })
}

/// A flat-bottom cloud in unit coordinates, with its baseline at y=0.
#[must_use]
pub fn cloud(scale: f32) -> Vec<PathEl> {
    let pt = |x, y| pt(x * scale, y * scale);
    vec![
        PathEl::MoveTo(pt(-0.5, 0.0)),
        PathEl::CubicTo {
            c1: pt(-0.46, -0.58),
            c2: pt(-0.30, -0.62),
            to: pt(-0.22, -0.52),
        },
        PathEl::CubicTo {
            c1: pt(-0.14, -1.05),
            c2: pt(0.14, -1.05),
            to: pt(0.22, -0.48),
        },
        PathEl::CubicTo {
            c1: pt(0.34, -0.56),
            c2: pt(0.46, -0.36),
            to: pt(0.5, 0.0),
        },
        PathEl::Close,
    ]
}

/// The crescent centred at the origin, scaled uniformly by the drawing client.
#[must_use]
pub fn moon(radius: f32) -> Vec<PathEl> {
    let s = radius / 13.0;
    let start = pt(6.0 * s, -radius);
    vec![
        PathEl::MoveTo(start),
        PathEl::ArcTo {
            radius,
            large_arc: true,
            sweep: false,
            to: pt(12.0 * s, 10.0 * s),
        },
        PathEl::ArcTo {
            radius: 10.5 * s,
            large_arc: false,
            sweep: true,
            to: start,
        },
        PathEl::Close,
    ]
}

/// Scene height at rest, in dp.
#[must_use]
pub const fn rest_height(inner: bool) -> f32 {
    if inner {
        HEADER_SCENE_INNER
    } else {
        HEADER_SCENE_COVER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::phone::{HEADER_REACH_EXPAND, HEADER_REACH_TEXT_SHARE};

    #[test]
    fn sunrise_and_peak_at_fold_sizes_and_reach() {
        let cover = rest(412.0, false);
        let inner = rest(437.5, true);
        let expanded = reach(412.0);
        let tall = rest_height(false) + HEADER_REACH_EXPAND * (1.0 - HEADER_REACH_TEXT_SHARE);
        assert_eq!(
            cover.arc.point(412.0, rest_height(false), 0.0),
            pt(36.0, 42.0)
        );
        assert_eq!(
            inner.arc.point(437.5, rest_height(true), 0.0),
            pt(40.0, 46.0)
        );
        assert_eq!(expanded.arc.point(412.0, tall, 0.0), pt(36.0, 147.0));
        assert_eq!(expanded.arc.point(412.0, tall, 0.5), pt(206.0, 44.0));
        assert_eq!(expanded.hills.map(|h| h.depth), [84.0, 58.0, 28.0]);
    }
}
