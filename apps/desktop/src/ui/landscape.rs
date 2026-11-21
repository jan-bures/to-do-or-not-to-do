//! The header landscape: the sky strip with two flat hills and the sun on its arc.
//!
//! A handful of single-colour paths, built from `geometry` and cached per strip bounds and progress. They are
//! rebuilt when the window is resized or the count changes, never per frame. No clipping, blur or masks.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, Hsla, LineCap, LineJoin, Path, PathBuilder, PathStyle, Pixels, StrokeOptions, Window,
    canvas, fill, point, prelude::*, px,
};

use crate::geometry::{self, Pt, STARS, sun};
use crate::theme::{Palette, with_alpha};

/// Which colour a cached path is painted with; resolved against the palette at paint time, so switching
/// themes doesn't rebuild paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Star,
    Arc,
    Travelled,
    SunHalo,
    Sun,
    HillFar,
    HillNear,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    bounds: Bounds<Pixels>,
    done: u32,
    total: u32,
    /// Scenery only: no arc and no sun (an empty list has nothing to keep score of).
    scenery: bool,
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

fn to_px(origin: gpui::Point<Pixels>, p: Pt) -> gpui::Point<Pixels> {
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

fn polyline(origin: gpui::Point<Pixels>, pts: &[Pt], width: f32) -> Option<Path> {
    let (first, rest) = pts.split_first()?;
    let mut b = stroke(width);
    b.move_to(to_px(origin, *first));
    for p in rest {
        b.line_to(to_px(origin, *p));
    }
    b.build().ok()
}

fn circle(origin: gpui::Point<Pixels>, c: Pt, r: f32) -> Option<Path> {
    let mut b = PathBuilder::fill();
    let r_px = px(r);
    b.move_to(to_px(origin, geometry::pt(c.x + r, c.y)));
    b.arc_to(
        point(r_px, r_px),
        px(0.),
        false,
        true,
        to_px(origin, geometry::pt(c.x - r, c.y)),
    );
    b.arc_to(
        point(r_px, r_px),
        px(0.),
        false,
        true,
        to_px(origin, geometry::pt(c.x + r, c.y)),
    );
    b.close();
    b.build().ok()
}

fn hill(origin: gpui::Point<Pixels>, hill: &geometry::Hill, w: f32, h: f32) -> Option<Path> {
    let mut b = PathBuilder::fill();
    b.move_to(to_px(origin, geometry::pt(0.0, h)));
    b.line_to(to_px(origin, hill.start));
    for c in &hill.curves {
        b.cubic_bezier_to(
            to_px(origin, c.to),
            to_px(origin, c.c1),
            to_px(origin, c.c2),
        );
    }
    b.line_to(to_px(origin, geometry::pt(w, h)));
    b.close();
    b.build().ok()
}

fn build(key: Key) -> Vec<(Path, Ink)> {
    let origin = key.bounds.origin;
    let (w, h) = (
        f32::from(key.bounds.size.width),
        f32::from(key.bounds.size.height),
    );
    let mut out = Vec::new();
    let mut push = |path: Option<Path>, ink| {
        if let Some(path) = path {
            out.push((path, ink));
        }
    };
    if key.stars {
        for (fx, fy, r) in STARS {
            push(circle(origin, geometry::pt(w * fx, h * fy), r), Ink::Star);
        }
    }
    if !key.scenery {
        let (travelled, remaining) = geometry::split_arc(key.done, key.total, w, h);
        push(polyline(origin, &remaining, 1.0), Ink::Arc);
        push(polyline(origin, &travelled, 1.5), Ink::Travelled);
        let s = geometry::sun_position(key.done, key.total, w, h);
        push(circle(origin, s, sun::HALO), Ink::SunHalo);
        for i in 0..sun::RAYS {
            let a = i as f32 * std::f32::consts::TAU / sun::RAYS as f32;
            let (dx, dy) = (a.sin(), -a.cos());
            let r0 = sun::RAY_OUTER - sun::RAY_LEN;
            let ray = [
                geometry::pt(s.x + dx * r0, s.y + dy * r0),
                geometry::pt(s.x + dx * sun::RAY_OUTER, s.y + dy * sun::RAY_OUTER),
            ];
            push(polyline(origin, &ray, sun::RAY_WIDTH), Ink::Sun);
        }
        push(circle(origin, s, sun::DISC), Ink::Sun);
    }
    let (far, near) = geometry::hills(w, h);
    push(hill(origin, &far, w, h), Ink::HillFar);
    push(hill(origin, &near, w, h), Ink::HillNear);
    out
}

fn color(p: &Palette, ink: Ink) -> Hsla {
    match ink {
        Ink::Star => with_alpha(p.star.unwrap_or(p.sky), 0.7),
        Ink::Arc => p.arc,
        Ink::Travelled => p.sun_ink,
        Ink::SunHalo => with_alpha(p.sun, 0.22),
        Ink::Sun => p.sun,
        Ink::HillFar => p.hill_far,
        Ink::HillNear => p.hill_near,
    }
}

/// The landscape strip. It fills its parent's width; give it the strip's height.
pub fn landscape(
    cache: &LandscapeCache,
    palette: Palette,
    done: u32,
    total: u32,
    scenery: bool,
) -> impl IntoElement {
    let cache = cache.clone();
    let stars = palette.star.is_some();
    canvas(
        move |bounds, _window, _cx| {
            let key = Key {
                bounds,
                done,
                total,
                scenery,
                stars,
            };
            let mut cached = cache.0.borrow_mut();
            if cached.key != Some(key) {
                cached.paths = build(key);
                cached.key = Some(key);
            }
            cached.paths.clone()
        },
        move |bounds, paths, window: &mut Window, _cx| {
            window.paint_quad(fill(bounds, palette.sky));
            if let Some(horizon) = palette.horizon {
                let top = geometry::horizon_top(f32::from(bounds.size.height));
                let band = Bounds::new(
                    point(bounds.origin.x, bounds.origin.y + px(top)),
                    gpui::size(bounds.size.width, bounds.size.height - px(top)),
                );
                window.paint_quad(fill(band, horizon));
            }
            for (path, ink) in paths {
                window.paint_path(path, color(&palette, ink));
            }
        },
    )
    .w_full()
    .h(px(crate::theme::space::LAND_H))
}
