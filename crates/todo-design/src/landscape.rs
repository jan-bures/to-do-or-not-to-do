//! The landscape under each room header: a sky in the room's hue, two flat hills, and a sun that climbs a faint
//! arc as tasks are done. The sun has one meaning, completed tasks, and it sits beside the matching count.
//!
//! This module computes the geometry, so GPUI, Compose and the web draw the same shapes. Everything is a
//! single-colour path or circle: no clipping, masks, blur or multicolour SVG. Build the paths once per size and
//! progress, never per frame.
//!
//! | Layer (back to front) | Slim (every day) | Finished (My Day done) |
//! |---|---|---|
//! | sky | `sky` fill | `dusk.top` → `dusk.low` vertical gradient |
//! | horizon band | dark only, `horizon`, from [`Landscape::horizon_top`] down | — |
//! | stars | dark only, `star` at [`STAR_OPACITY`] | `dusk.star` |
//! | moon | — | `dusk.star` at [`MOON_OPACITY`] |
//! | remaining arc | `arc`, [`ARC_STROKE`] | hidden |
//! | travelled arc | `sun-ink`, [`TRAVELLED_STROKE`] | `dusk.sun`, [`TRAVELLED_STROKE_FINISHED`] at 0.5 |
//! | sun | `sun`: halo, 8 rays, disc | `dusk.sun`, setting, with three rings |
//! | far hill, near hill | `hill-far`, `hill-near` | `dusk.silhouette-far`, `dusk.silhouette-near` |
//!
//! An empty list shows the hills only ([`Landscape::shows_sun`] is false): scenery, not progress.

use std::f32::consts::PI;
use std::fmt::Write as _;

pub mod phone;

/// A point in the landscape's own coordinates: origin top left, y down, logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// x
    pub x: f32,
    /// y
    pub y: f32,
}

const fn pt(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// One path command, with SVG semantics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathEl {
    /// Start a subpath.
    MoveTo(Point),
    /// Straight line.
    LineTo(Point),
    /// Cubic Bézier.
    CubicTo {
        /// First control point.
        c1: Point,
        /// Second control point.
        c2: Point,
        /// End point.
        to: Point,
    },
    /// Circular arc to `to` (SVG `A r r 0 large_arc sweep x y`).
    ArcTo {
        /// Radius.
        radius: f32,
        /// Take the longer of the two possible arcs.
        large_arc: bool,
        /// Sweep clockwise (in y-down coordinates).
        sweep: bool,
        /// End point.
        to: Point,
    },
    /// Close the subpath.
    Close,
}

/// Formats a path as SVG path data (Compose can read it with `PathParser`).
pub fn svg_path(path: &[PathEl]) -> String {
    let mut s = String::new();
    for el in path {
        // Writing to a String can't fail.
        let _ = match *el {
            PathEl::MoveTo(p) => write!(s, "M{:.1} {:.1}", p.x, p.y),
            PathEl::LineTo(p) => write!(s, "L{:.1} {:.1}", p.x, p.y),
            PathEl::CubicTo { c1, c2, to } => write!(
                s,
                "C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
                c1.x, c1.y, c2.x, c2.y, to.x, to.y
            ),
            PathEl::ArcTo {
                radius,
                large_arc,
                sweep,
                to,
            } => write!(
                s,
                "A{radius} {radius} 0 {} {} {:.1} {:.1}",
                u8::from(large_arc),
                u8::from(sweep),
                to.x,
                to.y
            ),
            PathEl::Close => write!(s, "Z"),
        };
    }
    s
}

/// Landscape height on the desktop.
pub const HEIGHT_DESKTOP: f32 = 48.0;
/// Landscape height on the phone cover screen.
pub const HEIGHT_COVER: f32 = 36.0;
/// Landscape height on the phone inner screen.
pub const HEIGHT_INNER: f32 = 40.0;
/// Height of the finished-day scene.
pub const HEIGHT_FINISHED: f32 = 164.0;

/// Stroke of the remaining arc.
pub const ARC_STROKE: f32 = 1.0;
/// Stroke of the travelled arc (round caps).
pub const TRAVELLED_STROKE: f32 = 1.5;
/// Stroke of the travelled arc on Android's taller horizon scene (round caps).
pub const TRAVELLED_STROKE_PHONE: f32 = 1.75;
/// Stroke of the travelled arc in the finished scene, drawn at 0.5 opacity.
pub const TRAVELLED_STROKE_FINISHED: f32 = 1.2;
/// Stroke of the sun's rays (round caps).
pub const RAY_STROKE: f32 = 1.6;
/// Stroke of the setting sun's rings.
pub const RING_STROKE: f32 = 1.2;
/// Opacity of the halo behind the sun disc.
pub const HALO_OPACITY: f32 = 0.22;
/// Opacity of the wider atmospheric glow behind the sun halo (pure circle, no blur).
pub const OUTER_GLOW_OPACITY: f32 = 0.08;
/// Opacity of the distant ridge hill layer in light theme (drawn in `hill-far`).
pub const RIDGE_OPACITY_LIGHT: f32 = 0.48;
/// Opacity of the distant ridge hill layer in dark theme (drawn in `hill-far`).
pub const RIDGE_OPACITY_DARK: f32 = 0.50;
/// Opacity of the distant ridge hill layer in the finished dusk scene (drawn in `dusk.silhouette-far`).
pub const RIDGE_OPACITY_DUSK: f32 = 0.45;
/// Opacity of calm flat-bottom daytime clouds in light theme (drawn in `star`).
pub const CLOUD_OPACITY: f32 = 0.48;
/// Opacity of stars.
pub const STAR_OPACITY: f32 = 0.7;
/// Opacity of subtle upper-sky zenith stars behind the top of a dark header.
pub const ZENITH_STAR_OPACITY: f32 = 0.30;
/// Opacity of the moon.
pub const MOON_OPACITY: f32 = 0.9;
/// Opacity of the subtle ambient crescent moon in dark-theme night and reach skies.
pub const AMBIENT_MOON_OPACITY: f32 = 0.30;

/// Which scene to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// The slim strip under every room header.
    Slim,
    /// The tall dusk scene when My Day is finished.
    Finished,
}

/// The sun, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Sun {
    /// Centre of the disc.
    pub center: Point,
    /// Disc radius.
    pub disc_radius: f32,
    /// Halo radius (drawn at [`HALO_OPACITY`]).
    pub halo_radius: f32,
    /// Eight rays as line segments (inner end, outer end), stroked at [`RAY_STROKE`].
    pub rays: [(Point, Point); 8],
    /// Rings around the setting sun: (radius, opacity). Empty in the slim scene.
    pub rings: Vec<(f32, f32)>,
}

/// A star: centre and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    /// Centre.
    pub center: Point,
    /// Radius.
    pub radius: f32,
}

const STARS: [(f32, f32, f32); 10] = [
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

/// Landscape geometry for one size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landscape {
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Scene.
    pub scene: Scene,
}

impl Landscape {
    /// The slim strip, `height` usually [`HEIGHT_DESKTOP`], [`HEIGHT_COVER`] or [`HEIGHT_INNER`].
    pub const fn slim(width: f32, height: f32) -> Landscape {
        Landscape {
            width,
            height,
            scene: Scene::Slim,
        }
    }

    /// The finished-day scene, [`HEIGHT_FINISHED`] tall.
    pub const fn finished(width: f32) -> Landscape {
        Landscape {
            width,
            height: HEIGHT_FINISHED,
            scene: Scene::Finished,
        }
    }

    fn big(&self) -> bool {
        self.scene == Scene::Finished
    }

    /// Hill amplitude factor.
    fn amp(&self) -> f32 {
        if self.big() { 3.0 } else { 1.0 }
    }

    fn arc_ends(&self) -> (f32, f32) {
        let pad = if self.big() { 64.0 } else { 28.0 };
        (pad, self.width - pad)
    }

    /// y of the arc's two ends.
    pub fn arc_base(&self) -> f32 {
        self.height - if self.big() { 64.0 } else { 13.0 }
    }

    fn arc_peak(&self) -> f32 {
        if self.big() { 46.0 } else { 10.0 }
    }

    /// Whether to draw the arc and the sun: only when the room has tasks. An empty list shows hills only.
    pub const fn shows_sun(total: usize) -> bool {
        total > 0
    }

    /// Progress along the arc for `done` of `total` tasks, 0–1.
    pub fn progress(done: usize, total: usize) -> f32 {
        if total == 0 {
            0.0
        } else {
            (done.min(total) as f32) / (total as f32)
        }
    }

    /// The point at progress `t` (0–1) on the sun's arc: a half sine from the left end to the right end.
    pub fn arc_point(&self, t: f32) -> Point {
        let (x0, x1) = self.arc_ends();
        let base = self.arc_base();
        let t = t.clamp(0.0, 1.0);
        pt(
            x0 + (x1 - x0) * t,
            base - (base - self.arc_peak()) * (PI * t).sin(),
        )
    }

    fn polyline(&self, from: f32, to: f32, segments: usize) -> Vec<Point> {
        (0..=segments)
            .map(|i| self.arc_point(from + (to - from) * i as f32 / segments as f32))
            .collect()
    }

    /// The travelled part of the arc, from the left end to the sun, as a polyline.
    pub fn travelled(&self, progress: f32) -> Vec<Point> {
        self.polyline(0.0, progress, 48)
    }

    /// The remaining part of the arc, from the sun to the right end, as a polyline. Not drawn in the finished
    /// scene.
    pub fn remaining(&self, progress: f32) -> Vec<Point> {
        self.polyline(progress, 1.0, 64)
    }

    /// The sun at `progress`, or at rest behind the hills for the finished scene.
    pub fn sun(&self, progress: f32) -> Sun {
        self.travelling_sun(if self.big() { 1.0 } else { progress })
    }

    /// The sun travelling along the arc during a transition, including the finished scene.
    pub fn travelling_sun(&self, progress: f32) -> Sun {
        let scale = if self.big() { 2.2 } else { 1.0 };
        let progress = progress.clamp(0.0, 1.0);
        let mut center = self.arc_point(progress);
        if self.big() {
            // Ease into the exact original setting position at the end of the finished arc.
            if progress == 1.0 {
                let (_, x1) = self.arc_ends();
                center = pt(x1 - 6.0, self.arc_base() + 10.0);
            } else {
                let setting = progress.powi(8);
                center.x -= 6.0 * setting;
                center.y += 10.0 * setting;
            }
        }
        let rays = std::array::from_fn(|i| {
            let a = (i as f32 * 45.0).to_radians();
            let at = |r: f32| pt(center.x + r * a.sin(), center.y - r * a.cos());
            (at(8.5 * scale), at(11.5 * scale))
        });
        let rings = if self.big() {
            vec![(44.0, 0.32), (64.0, 0.18), (86.0, 0.09)]
        } else {
            Vec::new()
        };
        Sun {
            center,
            disc_radius: 6.2 * scale,
            halo_radius: 11.0 * scale,
            rays,
            rings,
        }
    }

    /// A hill: a line up the left edge, three cubic curves across, and back down the right edge. `pts` holds
    /// each curve's two control points and end point as (fraction of width, y offset in amplitude units from
    /// `top`).
    fn hill(&self, top: f32, pts: [(f32, f32); 9]) -> Vec<PathEl> {
        let (w, h, a) = (self.width, self.height, self.amp());
        let p = |(fx, dy): (f32, f32)| pt(w * fx, top + dy * a);
        let mut path = vec![PathEl::MoveTo(pt(0.0, h)), PathEl::LineTo(pt(0.0, top))];
        for c in pts.as_chunks::<3>().0 {
            path.push(PathEl::CubicTo {
                c1: p(c[0]),
                c2: p(c[1]),
                to: p(c[2]),
            });
        }
        path.extend([PathEl::LineTo(pt(w, h)), PathEl::Close]);
        path
    }

    fn far_top(&self) -> f32 {
        self.height - if self.big() { 58.0 } else { 15.0 }
    }

    fn near_top(&self) -> f32 {
        self.height - if self.big() { 26.0 } else { 7.0 }
    }

    /// The far hill, a closed path.
    pub fn far_hill(&self) -> Vec<PathEl> {
        self.hill(self.far_top(), phone::FAR_POINTS)
    }

    /// The near hill, a closed path.
    pub fn near_hill(&self) -> Vec<PathEl> {
        self.hill(self.near_top(), phone::NEAR_POINTS)
    }

    /// Top of the horizon band drawn under the hills in the dark slim scene.
    pub fn horizon_top(&self) -> f32 {
        self.arc_base() - 12.0
    }

    /// The stars. Drawn in the dark slim scene and in the finished scene.
    pub fn stars(&self) -> Vec<Star> {
        let (ys, rs) = if self.big() { (0.6, 1.4) } else { (1.0, 1.0) };
        STARS
            .iter()
            .map(|&(x, y, r)| Star {
                center: pt(self.width * x, self.height * y * ys),
                radius: r * rs,
            })
            .collect()
    }

    /// The crescent moon of the finished scene, a closed path. `None` in the slim scene.
    pub fn moon(&self) -> Option<Vec<PathEl>> {
        if !self.big() {
            return None;
        }
        let (mx, my) = (self.width * 0.16, 58.0);
        let start = pt(mx + 6.0, my - 13.0);
        Some(vec![
            PathEl::MoveTo(start),
            PathEl::ArcTo {
                radius: 13.0,
                large_arc: true,
                sweep: false,
                to: pt(start.x + 6.0, start.y + 23.0),
            },
            PathEl::ArcTo {
                radius: 10.5,
                large_arc: false,
                sweep: true,
                to: start,
            },
            PathEl::Close,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_ends_and_peak() {
        let l = Landscape::slim(800.0, HEIGHT_DESKTOP);
        assert_eq!(l.arc_point(0.0), pt(28.0, 35.0));
        let end = l.arc_point(1.0);
        assert!((end.x - 772.0).abs() < 1e-3 && (end.y - 35.0).abs() < 1e-3);
        let mid = l.arc_point(0.5);
        assert!((mid.y - 10.0).abs() < 1e-3);
    }

    #[test]
    fn progress_is_clamped() {
        assert_eq!(Landscape::progress(3, 12), 0.25);
        assert_eq!(Landscape::progress(0, 0), 0.0);
        assert_eq!(Landscape::progress(13, 12), 1.0);
        assert!(!Landscape::shows_sun(0));
    }

    #[test]
    fn travelled_meets_remaining_at_the_sun() {
        let l = Landscape::slim(800.0, HEIGHT_DESKTOP);
        let u = Landscape::progress(3, 12);
        let sun = l.sun(u).center;
        assert_eq!(l.travelled(u).last().copied(), Some(sun));
        assert_eq!(l.remaining(u).first().copied(), Some(sun));
    }

    #[test]
    fn the_dusk_sun_travels_but_keeps_its_original_resting_place() {
        let land = Landscape::finished(720.0);
        let start = land.travelling_sun(0.0).center;
        let middle = land.travelling_sun(0.5).center;
        let end = land.travelling_sun(1.0).center;
        assert!(start.x < middle.x && middle.x < end.x);
        assert!(middle.y < end.y);
        assert_eq!(end, land.sun(1.0).center);
        assert_eq!(land.sun(0.5).center, end);
    }

    #[test]
    fn mockup_paths() {
        // The desktop strip in the approved mockup (800 × 48).
        let l = Landscape::slim(800.0, 48.0);
        assert_eq!(
            svg_path(&l.far_hill()),
            "M0.0 48.0L0.0 33.0C96.0 27.0 176.0 36.0 288.0 31.0C400.0 26.0 496.0 25.0 592.0 32.0\
             C688.0 39.0 744.0 35.0 800.0 29.0L800.0 48.0Z"
        );
        assert_eq!(
            svg_path(&l.near_hill()),
            "M0.0 48.0L0.0 41.0C120.0 36.0 240.0 44.0 384.0 40.0C528.0 36.0 608.0 36.0 720.0 41.0\
             C832.0 46.0 784.0 42.0 800.0 39.0L800.0 48.0Z"
        );
        let f = Landscape::finished(720.0);
        assert_eq!(
            f.moon().map(|m| svg_path(&m)),
            Some("M121.2 45.0A13 13 0 1 0 127.2 68.0A10.5 10.5 0 0 1 121.2 45.0Z".to_owned())
        );
        let sun = f.sun(1.0);
        assert_eq!(sun.center, pt(650.0, 110.0));
        assert_eq!(sun.rings.len(), 3);
    }
}
