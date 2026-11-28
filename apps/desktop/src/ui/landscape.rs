//! The header landscape: the sky with two flat hills and the sun on its arc, or the finished day's dusk.
//!
//! The geometry is `todo_design::landscape::Landscape`, the same shapes Compose and the web draw. Here it
//! becomes a handful of single-colour GPUI paths, cached per bounds and the sun's place: they are rebuilt
//! when the window is resized or the sun moves (every frame only while it travels to a new count, see
//! `model::motion`), and never at rest. No clipping, blur or masks.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, Hsla, LineCap, LineJoin, Path, PathBuilder, PathStyle, Pixels, StrokeOptions, Window,
    canvas, fill, linear_color_stop, linear_gradient, point, prelude::*, px,
};

use crate::theme::landscape::{
    self as geo, ARC_STROKE, HALO_OPACITY, Landscape, MOON_OPACITY, PathEl, RAY_STROKE,
    RING_STROKE, STAR_OPACITY, Scene, TRAVELLED_STROKE, TRAVELLED_STROKE_FINISHED,
};
use crate::theme::{Dusk, Palette, mix, with_alpha};

/// Which colour a cached path is painted with. Resolved against the palette at paint time, so switching
/// themes doesn't rebuild paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Star,
    Moon,
    Arc,
    Travelled,
    SunHalo,
    SunRing(u8),
    Sun,
    HillFar,
    HillNear,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    bounds: Bounds<Pixels>,
    /// The sun's place on its arc, 0 to 1, or `None` for hills only.
    sun: Option<f32>,
    scene: Scene,
    stars: bool,
}

#[derive(Default)]
struct Cached {
    key: Option<Key>,
    paths: Vec<(Path, Ink)>,
}

/// Cache shared between frames. Keep one per header.
#[derive(Clone, Default)]
pub struct LandscapeCache(Rc<RefCell<Cached>>);

impl std::fmt::Debug for LandscapeCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LandscapeCache").finish_non_exhaustive()
    }
}

/// What the header shows.
#[derive(Clone, Copy, Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "made once per header render and moved into its paint closure; a box would only add an allocation"
)]
pub enum Sky {
    /// The everyday strip in the room's palette.
    Slim(Palette),
    /// My Day is done: the tall dusk scene.
    Finished(Dusk),
}

type Origin = gpui::Point<Pixels>;

fn at(origin: Origin, p: geo::Point) -> Origin {
    point(origin.x + px(p.x), origin.y + px(p.y))
}

fn stroke(width: f32) -> PathBuilder {
    PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(StrokeOptions {
        line_width: width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..StrokeOptions::default()
    }))
}

fn polyline(origin: Origin, pts: &[geo::Point], width: f32) -> Option<Path> {
    let (first, rest) = pts.split_first()?;
    let mut b = stroke(width);
    b.move_to(at(origin, *first));
    for p in rest {
        b.line_to(at(origin, *p));
    }
    b.build().ok()
}

fn circle_into(mut b: PathBuilder, origin: Origin, c: geo::Point, r: f32) -> Option<Path> {
    let radii = point(px(r), px(r));
    let east = at(origin, geo::Point { x: c.x + r, y: c.y });
    let west = at(origin, geo::Point { x: c.x - r, y: c.y });
    b.move_to(east);
    b.arc_to(radii, px(0.), false, true, west);
    b.arc_to(radii, px(0.), false, true, east);
    b.close();
    b.build().ok()
}

fn disc(origin: Origin, c: geo::Point, r: f32) -> Option<Path> {
    circle_into(PathBuilder::fill(), origin, c, r)
}

fn ring(origin: Origin, c: geo::Point, r: f32, width: f32) -> Option<Path> {
    circle_into(stroke(width), origin, c, r)
}

/// A closed shape from the design's path elements.
fn shape(origin: Origin, els: &[PathEl]) -> Option<Path> {
    let mut b = PathBuilder::fill();
    for el in els {
        match *el {
            PathEl::MoveTo(p) => b.move_to(at(origin, p)),
            PathEl::LineTo(p) => b.line_to(at(origin, p)),
            PathEl::CubicTo { c1, c2, to } => {
                b.cubic_bezier_to(at(origin, to), at(origin, c1), at(origin, c2));
            }
            PathEl::ArcTo {
                radius,
                large_arc,
                sweep,
                to,
            } => b.arc_to(
                point(px(radius), px(radius)),
                px(0.),
                large_arc,
                sweep,
                at(origin, to),
            ),
            PathEl::Close => b.close(),
        }
    }
    b.build().ok()
}

fn build(key: Key) -> Vec<(Path, Ink)> {
    let origin = key.bounds.origin;
    let width = f32::from(key.bounds.size.width);
    let height = f32::from(key.bounds.size.height);
    let land = match key.scene {
        Scene::Slim => Landscape::slim(width, height),
        Scene::Finished => Landscape::finished(width),
    };
    let finished = key.scene == Scene::Finished;
    let mut out = Vec::new();
    let mut push = |path: Option<Path>, ink| {
        if let Some(path) = path {
            out.push((path, ink));
        }
    };
    if key.stars || finished {
        for s in land.stars() {
            push(disc(origin, s.center, s.radius), Ink::Star);
        }
    }
    if let Some(moon) = land.moon() {
        push(shape(origin, &moon), Ink::Moon);
    }
    if let Some(progress) = key.sun.or(finished.then_some(1.0)) {
        if finished {
            push(
                polyline(origin, &land.travelled(progress), TRAVELLED_STROKE_FINISHED),
                Ink::Travelled,
            );
        } else {
            push(
                polyline(origin, &land.remaining(progress), ARC_STROKE),
                Ink::Arc,
            );
            push(
                polyline(origin, &land.travelled(progress), TRAVELLED_STROKE),
                Ink::Travelled,
            );
        }
        let sun = land.travelling_sun(progress);
        for (i, (r, _)) in sun.rings.iter().enumerate() {
            push(
                ring(origin, sun.center, *r, RING_STROKE),
                Ink::SunRing(i as u8),
            );
        }
        push(disc(origin, sun.center, sun.halo_radius), Ink::SunHalo);
        for (a, b) in sun.rays {
            push(polyline(origin, &[a, b], RAY_STROKE), Ink::Sun);
        }
        push(disc(origin, sun.center, sun.disc_radius), Ink::Sun);
    }
    push(shape(origin, &land.far_hill()), Ink::HillFar);
    push(shape(origin, &land.near_hill()), Ink::HillNear);
    out
}

fn ring_opacity(i: u8) -> f32 {
    // The setting sun's rings fade outwards; the opacities are the design's.
    Landscape::finished(0.0)
        .sun(1.0)
        .rings
        .get(usize::from(i))
        .map_or(0.0, |r| r.1)
}

fn color(sky: &Sky, ink: Ink) -> Hsla {
    match sky {
        Sky::Slim(p) => match ink {
            Ink::Star | Ink::Moon => with_alpha(p.star, STAR_OPACITY),
            Ink::Arc => p.arc,
            Ink::Travelled => p.sun_ink,
            Ink::SunHalo => with_alpha(p.sun, HALO_OPACITY),
            Ink::SunRing(i) => with_alpha(p.sun, ring_opacity(i)),
            Ink::Sun => p.sun,
            Ink::HillFar => p.hill_far,
            Ink::HillNear => p.hill_near,
        },
        Sky::Finished(d) => match ink {
            Ink::Star => with_alpha(d.star, STAR_OPACITY),
            Ink::Moon => with_alpha(d.star, MOON_OPACITY),
            Ink::Arc => d.sun,
            // The finished scene's travelled path is drawn at half opacity.
            Ink::Travelled => with_alpha(d.sun, 0.5),
            Ink::SunHalo => with_alpha(d.sun, HALO_OPACITY),
            Ink::SunRing(i) => with_alpha(d.sun, ring_opacity(i)),
            Ink::Sun => d.sun,
            Ink::HillFar => d.silhouette_far,
            Ink::HillNear => d.silhouette_near,
        },
    }
}

/// Where `done` of `total` puts the sun on its arc, or `None` when there's nothing to keep score of.
pub fn sun_at(done: u32, total: u32) -> Option<f32> {
    Landscape::shows_sun(total as usize).then(|| Landscape::progress(done as usize, total as usize))
}

impl LandscapeCache {
    /// The scene's paths for `key`, built only when it changed since the last frame.
    fn paths(&self, key: Key) -> Vec<(Path, Ink)> {
        let mut cached = self.0.borrow_mut();
        if cached.key != Some(key) {
            cached.paths = build(key);
            cached.key = Some(key);
        }
        cached.paths.clone()
    }
}

/// A rectangle `height` tall at the bottom of `bounds`: where a scene sits while the header grows.
fn bottom(bounds: Bounds<Pixels>, height: f32) -> Bounds<Pixels> {
    let height = px(height);
    Bounds::new(
        point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height - height,
        ),
        gpui::size(bounds.size.width, height),
    )
}

/// The everyday strip's sky in `bounds`, with the dark theme's horizon band behind the strip at the bottom.
/// The sky and horizon quads are split cleanly so translucent acrylic surfaces never double-blend.
fn paint_strip_sky(window: &mut Window, bounds: Bounds<Pixels>, p: &Palette) {
    if p.night() {
        let strip = bottom(bounds, crate::theme::desktop::LANDSCAPE);
        let land = Landscape::slim(f32::from(strip.size.width), f32::from(strip.size.height));
        let split_y =
            (strip.origin.y + px(land.horizon_top())).clamp(bounds.origin.y, bounds.bottom());
        let upper_h = split_y - bounds.origin.y;
        if upper_h > px(0.) {
            let upper = Bounds::new(bounds.origin, gpui::size(bounds.size.width, upper_h));
            window.paint_quad(fill(upper, p.surface_sky()));
        }
        let lower_h = bounds.bottom() - split_y;
        if lower_h > px(0.) {
            let band = Bounds::new(
                point(bounds.origin.x, split_y),
                gpui::size(bounds.size.width, lower_h),
            );
            window.paint_quad(fill(band, p.surface_horizon()));
        }
    } else {
        window.paint_quad(fill(bounds, p.surface_sky()));
    }
}

/// The dusk gradient in `bounds`, at `alpha`.
fn paint_dusk_sky(window: &mut Window, bounds: Bounds<Pixels>, d: &Dusk, alpha: f32) {
    window.paint_quad(fill(
        bounds,
        linear_gradient(
            180.,
            linear_color_stop(faded(d.top, alpha), 0.),
            linear_color_stop(faded(d.low, alpha), 1.),
        ),
    ));
}

fn faded(c: Hsla, alpha: f32) -> Hsla {
    with_alpha(c, c.a * alpha)
}

/// The landscape, with the sun at `sun` on its arc (0 to 1; `None` for a room without tasks, which shows
/// hills only). It fills its parent's width; its height is the scene's (`desktop::LANDSCAPE` or
/// `landscape::HEIGHT_FINISHED`). In the finished scene the dusk gradient also runs behind the header text,
/// so the parent paints `dusk.top` above it.
pub fn landscape(cache: &LandscapeCache, sky: Sky, sun: Option<f32>) -> impl IntoElement {
    let cache = cache.clone();
    let (scene, stars, height) = match sky {
        Sky::Slim(p) => (Scene::Slim, p.night(), crate::theme::desktop::LANDSCAPE),
        Sky::Finished(_) => (Scene::Finished, true, geo::HEIGHT_FINISHED),
    };
    canvas(
        move |bounds, _window, _cx| {
            cache.paths(Key {
                bounds,
                sun,
                scene,
                stars,
            })
        },
        move |bounds, paths, window: &mut Window, _cx| {
            match sky {
                Sky::Slim(p) => paint_strip_sky(window, bounds, &p),
                Sky::Finished(d) => paint_dusk_sky(window, bounds, &d, 1.),
            }
            for (path, ink) in paths {
                window.paint_path(path, color(&sky, ink));
            }
        },
    )
    .w_full()
    .h(px(height))
}

/// The room header's caches: one per scene, so a dusk that's falling draws both without rebuilding
/// either at rest.
#[derive(Clone, Debug, Default)]
pub struct HeaderCache {
    strip: LandscapeCache,
    dusk: LandscapeCache,
}

/// Where the room header is between its everyday strip and the dusk scene (see
/// `model::motion::DuskLook`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    /// How far the header has grown from `desktop::LANDSCAPE` to `landscape::HEIGHT_FINISHED`.
    pub height: f32,
    /// How far the dusk scene has faded in over the strip.
    pub scene: f32,
    /// How far the moon and stars have faded in.
    pub night: f32,
    /// Opacity of the slim scene's sun and arc when switching to or from an empty room.
    pub sun_alpha: f32,
}

/// The room header's landscape: the everyday strip in `palette`, the finished day's dusk scene, or (while
/// dusk falls or lifts) the dusk scene fading in over the strip as the header grows. Each scene sits at
/// the bottom of the growing header, so the hills stay on the ground; the dusk gradient spans the whole
/// height, matching the colour the parent paints above it (`dusk.top` blended over `sky` by
/// `blend.scene`). At rest it draws exactly what [`landscape`] does.
pub fn header_landscape(
    cache: &HeaderCache,
    palette: Palette,
    dusk: Dusk,
    sun: Option<f32>,
    blend: Blend,
) -> impl IntoElement {
    let cache = cache.clone();
    let slim = crate::theme::desktop::LANDSCAPE;
    let height = slim + (geo::HEIGHT_FINISHED - slim) * blend.height;
    canvas(
        move |bounds, _window, _cx| {
            let strip = (blend.scene < 1.).then(|| {
                cache.strip.paths(Key {
                    bounds: bottom(bounds, slim),
                    sun,
                    scene: Scene::Slim,
                    stars: palette.night(),
                })
            });
            let night = (blend.scene > 0.).then(|| {
                cache.dusk.paths(Key {
                    bounds: bottom(bounds, geo::HEIGHT_FINISHED),
                    sun,
                    scene: Scene::Finished,
                    stars: true,
                })
            });
            (strip, night)
        },
        move |bounds, (strip, night), window: &mut Window, _cx| {
            // Paint the sky once (never double-blending two translucent skies during a transition)
            // so its top edge matches the header's `sky_bg` pixel-for-pixel at every frame.
            if blend.scene <= 0. {
                paint_strip_sky(window, bounds, &palette);
            } else if blend.scene >= 1. {
                paint_dusk_sky(window, bounds, &dusk, palette.dusk_alpha());
            } else {
                let top = mix(
                    palette.surface_sky(),
                    with_alpha(dusk.top, palette.dusk_alpha()),
                    blend.scene,
                );
                let strip_low = if palette.night() {
                    palette.surface_horizon()
                } else {
                    palette.surface_sky()
                };
                let low = mix(
                    strip_low,
                    with_alpha(dusk.low, palette.dusk_alpha()),
                    blend.scene,
                );
                window.paint_quad(fill(
                    bounds,
                    linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(low, 1.)),
                ));
            }
            if let Some(paths) = strip {
                let sky = Sky::Slim(palette);
                let fade = 1. - blend.scene;
                for (path, ink) in paths {
                    let alpha = match ink {
                        Ink::Arc | Ink::Travelled | Ink::SunHalo | Ink::SunRing(_) | Ink::Sun => {
                            blend.sun_alpha * fade
                        }
                        _ => fade,
                    };
                    window.paint_path(path, faded(color(&sky, ink), alpha));
                }
            }
            if let Some(paths) = night {
                let sky = Sky::Finished(dusk);
                for (path, ink) in paths {
                    let alpha = match ink {
                        Ink::Star | Ink::Moon => blend.scene * blend.night,
                        _ => blend.scene,
                    };
                    window.paint_path(path, faded(color(&sky, ink), alpha));
                }
            }
        },
    )
    .w_full()
    .h(px(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_sits_at_done_over_total_on_the_mockup_strip() {
        // The F2 mockup's list column: 800 wide, sun at 3 of 12 done.
        let land = Landscape::slim(800.0, crate::theme::desktop::LANDSCAPE);
        let sun = land.sun(Landscape::progress(3, 12));
        assert!((sun.center.x - 214.0).abs() < 0.5, "{}", sun.center.x);
        // An empty list keeps score of nothing: hills only.
        assert!(!Landscape::shows_sun(0));
    }

    #[test]
    fn the_finished_rings_fade_outwards() {
        assert!(ring_opacity(0) > ring_opacity(1) && ring_opacity(1) > ring_opacity(2));
        assert!(ring_opacity(2) > 0.0);
        assert_eq!(ring_opacity(9), 0.0);
    }
}
