//! The header landscape for Compose: the same geometry the desktop draws (`todo_design::landscape`),
//! flattened into records. Closed shapes come as SVG path data (Compose reads it with `PathParser`), the
//! arc as polylines, the sun and stars as circles. Build it when the size or the count changes, never
//! per frame.

use todo_design::landscape::{
    self as geo, ARC_STROKE, HALO_OPACITY, Landscape, MOON_OPACITY, RAY_STROKE, RING_STROKE,
    STAR_OPACITY, TRAVELLED_STROKE, TRAVELLED_STROKE_FINISHED,
};

/// A point in the landscape's coordinates (dp, origin top left).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Pt {
    /// x
    pub x: f32,
    /// y
    pub y: f32,
}

impl From<geo::Point> for Pt {
    fn from(p: geo::Point) -> Self {
        Self { x: p.x, y: p.y }
    }
}

/// A filled circle.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Dot {
    /// Centre.
    pub center: Pt,
    /// Radius.
    pub radius: f32,
}

/// A ring around the setting sun.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Ring {
    /// Radius.
    pub radius: f32,
    /// Opacity.
    pub opacity: f32,
}

/// A line segment (a ray).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Segment {
    /// Inner end.
    pub from: Pt,
    /// Outer end.
    pub to: Pt,
}

/// The sun.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct SunShape {
    /// Centre.
    pub center: Pt,
    /// Disc radius.
    pub disc_radius: f32,
    /// Halo radius, drawn at `halo_opacity`.
    pub halo_radius: f32,
    /// Eight rays, stroked at `ray_stroke`.
    pub rays: Vec<Segment>,
    /// Rings (finished scene only).
    pub rings: Vec<Ring>,
}

/// Everything to draw, back to front: stars, moon, remaining arc, travelled arc, rings, halo, rays and
/// disc, far hill, near hill.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LandscapeShapes {
    /// Stars (dark slim scene and finished scene; empty otherwise).
    pub stars: Vec<Dot>,
    /// The crescent moon (finished scene).
    pub moon: Option<String>,
    /// The arc still to go, stroked at `arc_stroke` in `arc` (empty when hidden).
    pub remaining: Vec<Pt>,
    /// The travelled arc, stroked at `travelled_stroke` in `sun-ink` (or the dusk sun).
    pub travelled: Vec<Pt>,
    /// Travelled stroke width.
    pub travelled_stroke: f32,
    /// Remaining stroke width.
    pub arc_stroke: f32,
    /// The sun, when the room has tasks (or the day is finished).
    pub sun: Option<SunShape>,
    /// Far hill, SVG path data.
    pub far_hill: String,
    /// Near hill, SVG path data.
    pub near_hill: String,
    /// Top of the dark horizon band (slim scene).
    pub horizon_top: f32,
    /// Opacity of the halo.
    pub halo_opacity: f32,
    /// Opacity of stars.
    pub star_opacity: f32,
    /// Opacity of the moon.
    pub moon_opacity: f32,
    /// Ray stroke width.
    pub ray_stroke: f32,
    /// Ring stroke width.
    pub ring_stroke: f32,
}

fn pts(v: Vec<geo::Point>) -> Vec<Pt> {
    v.into_iter().map(Pt::from).collect()
}

/// The landscape for a header `width` × `height` dp (the height is ignored for the finished scene, which
/// is always 164). `stars` draws the dark theme's stars in the slim scene.
#[uniffi::export]
pub fn landscape(
    width: f32,
    height: f32,
    finished: bool,
    done: u32,
    total: u32,
    stars: bool,
) -> LandscapeShapes {
    let land = if finished {
        Landscape::finished(width)
    } else {
        Landscape::slim(width, height)
    };
    let progress = Landscape::progress(done as usize, total as usize);
    let shows_sun = finished || Landscape::shows_sun(total as usize);
    let sun = shows_sun.then(|| {
        let s = land.sun(progress);
        SunShape {
            center: s.center.into(),
            disc_radius: s.disc_radius,
            halo_radius: s.halo_radius,
            rays: s
                .rays
                .iter()
                .map(|(a, b)| Segment {
                    from: (*a).into(),
                    to: (*b).into(),
                })
                .collect(),
            rings: s
                .rings
                .iter()
                .map(|(radius, opacity)| Ring {
                    radius: *radius,
                    opacity: *opacity,
                })
                .collect(),
        }
    });
    let (remaining, travelled) = match (shows_sun, finished) {
        (false, _) => (Vec::new(), Vec::new()),
        (true, true) => (Vec::new(), pts(land.travelled(1.0))),
        (true, false) => (pts(land.remaining(progress)), pts(land.travelled(progress))),
    };
    LandscapeShapes {
        stars: if stars || finished {
            land.stars()
                .into_iter()
                .map(|s| Dot {
                    center: s.center.into(),
                    radius: s.radius,
                })
                .collect()
        } else {
            Vec::new()
        },
        moon: land.moon().map(|m| geo::svg_path(&m)),
        remaining,
        travelled,
        travelled_stroke: if finished {
            TRAVELLED_STROKE_FINISHED
        } else {
            TRAVELLED_STROKE
        },
        arc_stroke: ARC_STROKE,
        sun,
        far_hill: geo::svg_path(&land.far_hill()),
        near_hill: geo::svg_path(&land.near_hill()),
        horizon_top: land.horizon_top(),
        halo_opacity: HALO_OPACITY,
        star_opacity: STAR_OPACITY,
        moon_opacity: MOON_OPACITY,
        ray_stroke: RAY_STROKE,
        ring_stroke: RING_STROKE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_design::landscape::{HEIGHT_COVER, HEIGHT_FINISHED};

    #[test]
    fn the_sun_sits_on_the_arc_at_the_count() {
        let l = landscape(412.0, HEIGHT_COVER, false, 3, 12, false);
        let geo = Landscape::slim(412.0, HEIGHT_COVER);
        let sun = l.sun.unwrap();
        assert_eq!(sun.center, Pt::from(geo.arc_point(0.25)));
        assert_eq!(l.travelled.last().copied(), Some(sun.center));
        assert_eq!(l.remaining.first().copied(), Some(sun.center));
        assert_eq!(sun.rays.len(), 8);
        assert!(sun.rings.is_empty() && l.stars.is_empty() && l.moon.is_none());
        assert!(l.far_hill.starts_with('M') && l.near_hill.ends_with('Z'));
    }

    #[test]
    fn an_empty_room_shows_hills_only() {
        let l = landscape(412.0, HEIGHT_COVER, false, 0, 0, true);
        assert!(l.sun.is_none() && l.remaining.is_empty() && l.travelled.is_empty());
        assert_eq!(l.stars.len(), 10);
    }

    #[test]
    fn the_finished_day_sets_the_sun() {
        let l = landscape(412.0, 40.0, true, 12, 12, false);
        let sun = l.sun.unwrap();
        assert_eq!(sun.rings.len(), 3);
        assert!(l.moon.is_some() && l.remaining.is_empty() && !l.stars.is_empty());
        // Behind the hills, low in the 164 dp scene.
        assert!(sun.center.y > HEIGHT_FINISHED / 2.0);
        assert_eq!(l.travelled_stroke, TRAVELLED_STROKE_FINISHED);
    }
}
