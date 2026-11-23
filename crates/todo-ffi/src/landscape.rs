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

/// One phone hill's offset and amplitude at a scene endpoint.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct PhoneHill {
    /// Baseline above the bottom (dp).
    pub depth: f32,
    /// Multiplier for the Bézier control point offsets.
    pub amplitude: f32,
}

/// Dimensions that interpolate independently of the canvas's actual height.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PhonePanorama {
    /// Hill dimensions in drawing order.
    pub hills: Vec<PhoneHill>,
    /// Arc's horizontal inset (dp).
    pub pad: f32,
    /// Arc's baseline above the bottom (dp).
    pub base_depth: f32,
    /// Arc's peak below the top (dp).
    pub peak: f32,
}

impl From<geo::phone::Panorama> for PhonePanorama {
    fn from(p: geo::phone::Panorama) -> Self {
        Self {
            hills: p
                .hills
                .iter()
                .map(|h| PhoneHill {
                    depth: h.depth,
                    amplitude: h.amplitude,
                })
                .collect(),
            pad: p.arc.pad,
            base_depth: p.arc.base_depth,
            peak: p.arc.peak,
        }
    }
}

/// The cached endpoint geometry shared by phone room headers.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PhoneLandscape {
    /// Resting panorama.
    pub rest: PhonePanorama,
    /// Full reach panorama.
    pub reach: PhonePanorama,
    /// Finished panorama.
    pub dusk: PhonePanorama,
    /// Finished panorama at full reach.
    pub dusk_reach: PhonePanorama,
    /// Unit Bézier coordinates (ridge, far, near).
    pub hill_controls: Vec<Vec<Pt>>,
    /// Unit sky positions, with radii in dp.
    pub stars: Vec<Dot>,
    /// Cloud, SVG path data in a 100-unit space, centred at its baseline.
    pub cloud: String,
    /// Standard 9.5 dp ambient crescent, centred at the origin.
    pub ambient_moon: String,
    /// The finished day's 13 dp crescent, centred at the origin.
    pub dusk_moon: String,
}

/// Geometry depends on posture and pane width, never on the current count or drag distance.
#[uniffi::export]
pub fn phone_landscape(width: f32, inner: bool) -> PhoneLandscape {
    use geo::phone;
    PhoneLandscape {
        rest: phone::rest(width, inner).into(),
        reach: phone::reach(width).into(),
        dusk: phone::dusk(false).into(),
        dusk_reach: phone::dusk(true).into(),
        hill_controls: phone::hill_controls()
            .into_iter()
            .map(|points| points.into_iter().map(Pt::from).collect())
            .collect(),
        stars: phone::stars()
            .into_iter()
            .map(|s| Dot {
                center: s.center.into(),
                radius: s.radius,
            })
            .collect(),
        cloud: geo::svg_path(&phone::cloud(100.0)),
        ambient_moon: geo::svg_path(&phone::moon(9.5)),
        dusk_moon: geo::svg_path(&phone::moon(13.0)),
    }
}

/// RGB ink of the remaining arc over the full room sky gradient.
#[uniffi::export]
pub fn phone_arc_color(dark: bool, hue: &str) -> u32 {
    let theme = if dark {
        todo_design::Theme::Dark
    } else {
        todo_design::Theme::Light
    };
    let hue = todo_design::hues::by_key(hue).map_or(todo_design::hues::MY_DAY, |h| h.degrees);
    todo_design::palette::phone_arc(theme, hue).to_u32()
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
