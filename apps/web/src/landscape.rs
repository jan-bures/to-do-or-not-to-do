//! The header landscape as SVG markup, from `todo_design::landscape` (the same geometry the desktop app
//! paints). Colours are never written here: every shape carries a class, and `app.css` fills it from the
//! room's tokens (`--color-sky`, `--color-hill-far`, `--dusk-top`, ...). Stroke widths and opacities are
//! the design's constants, written as SVG presentation attributes (which the CSP allows; inline `style`
//! attributes it does not).

use std::fmt::Write as _;
use todo_design::landscape::{
    ARC_STROKE, HALO_OPACITY, Landscape, MOON_OPACITY, PathEl, Point, RAY_STROKE, RING_STROKE,
    STAR_OPACITY, TRAVELLED_STROKE, TRAVELLED_STROKE_FINISHED, svg_path,
};

/// What to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sky {
    /// The everyday strip, `height` tall; `night` adds the horizon band and stars (dark theme).
    Slim {
        /// Height in CSS pixels.
        height: f32,
        /// Dark theme.
        night: bool,
    },
    /// The finished day's dusk scene.
    Finished,
}

fn f(v: f32) -> String {
    // Two decimals are plenty at CSS-pixel scale and keep the markup small.
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

fn points(pts: &[Point]) -> String {
    pts.iter()
        .map(|p| format!("{},{}", f(p.x), f(p.y)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shape(out: &mut String, class: &str, els: &[PathEl]) {
    let _ = write!(out, r#"<path class="{class}" d="{}"/>"#, svg_path(els));
}

/// The SVG for a landscape `width` wide, with the sun at `progress` (0–1) when `total > 0`.
#[must_use]
pub fn svg(width: f32, sky: Sky, progress: f32, total: usize) -> String {
    let width = width.max(64.0);
    let (land, night, finished) = match sky {
        Sky::Slim { height, night } => (Landscape::slim(width, height), night, false),
        Sky::Finished => (Landscape::finished(width), true, true),
    };
    let h = land.height;
    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        r#"<svg class="ls" xmlns="http://www.w3.org/2000/svg" width="{w}" height="{hh}" viewBox="0 0 {w} {hh}" aria-hidden="true" focusable="false">"#,
        w = f(width),
        hh = f(h),
    );
    if finished {
        out.push_str(
            r#"<defs><linearGradient id="ls-dusk" x1="0" y1="0" x2="0" y2="1"><stop class="ls-dusk-top" offset="0"/><stop class="ls-dusk-low" offset="1"/></linearGradient></defs>"#,
        );
        let _ = write!(
            out,
            r#"<rect class="ls-dusk" width="{}" height="{}"/>"#,
            f(width),
            f(h)
        );
    } else {
        let _ = write!(
            out,
            r#"<rect class="ls-sky" width="{}" height="{}"/>"#,
            f(width),
            f(h)
        );
        if night {
            let top = land.horizon_top();
            let _ = write!(
                out,
                r#"<rect class="ls-horizon" y="{}" width="{}" height="{}"/>"#,
                f(top),
                f(width),
                f(h - top)
            );
        }
    }
    if night {
        for s in land.stars() {
            let _ = write!(
                out,
                r#"<circle class="ls-star" cx="{}" cy="{}" r="{}" opacity="{}"/>"#,
                f(s.center.x),
                f(s.center.y),
                f(s.radius),
                f(STAR_OPACITY)
            );
        }
    }
    if let Some(moon) = land.moon() {
        let _ = write!(
            out,
            r#"<path class="ls-star" d="{}" opacity="{}"/>"#,
            svg_path(&moon),
            f(MOON_OPACITY)
        );
    }
    if Landscape::shows_sun(total) || finished {
        let p = progress.clamp(0.0, 1.0);
        if finished {
            let _ = write!(
                out,
                r#"<polyline class="ls-travelled" points="{}" stroke-width="{}" opacity="0.5"/>"#,
                points(&land.travelled(1.0)),
                f(TRAVELLED_STROKE_FINISHED)
            );
        } else {
            let _ = write!(
                out,
                r#"<polyline class="ls-arc" points="{}" stroke-width="{}"/>"#,
                points(&land.remaining(p)),
                f(ARC_STROKE)
            );
            let _ = write!(
                out,
                r#"<polyline class="ls-travelled" points="{}" stroke-width="{}"/>"#,
                points(&land.travelled(p)),
                f(TRAVELLED_STROKE)
            );
        }
        let sun = land.sun(p);
        let (cx, cy) = (f(sun.center.x), f(sun.center.y));
        for (r, o) in &sun.rings {
            let _ = write!(
                out,
                r#"<circle class="ls-ring" cx="{cx}" cy="{cy}" r="{}" stroke-width="{}" opacity="{}"/>"#,
                f(*r),
                f(RING_STROKE),
                f(*o)
            );
        }
        let _ = write!(
            out,
            r#"<circle class="ls-sun" cx="{cx}" cy="{cy}" r="{}" opacity="{}"/>"#,
            f(sun.halo_radius),
            f(HALO_OPACITY)
        );
        for (a, b) in sun.rays {
            let _ = write!(
                out,
                r#"<line class="ls-ray" x1="{}" y1="{}" x2="{}" y2="{}" stroke-width="{}"/>"#,
                f(a.x),
                f(a.y),
                f(b.x),
                f(b.y),
                f(RAY_STROKE)
            );
        }
        let _ = write!(
            out,
            r#"<circle class="ls-sun" cx="{cx}" cy="{cy}" r="{}"/>"#,
            f(sun.disc_radius)
        );
    }
    shape(&mut out, "ls-hill-far", &land.far_hill());
    shape(&mut out, "ls-hill-near", &land.near_hill());
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_design::landscape::HEIGHT_DESKTOP;

    #[test]
    fn an_empty_room_shows_hills_only() {
        let s = svg(
            800.0,
            Sky::Slim {
                height: HEIGHT_DESKTOP,
                night: false,
            },
            0.0,
            0,
        );
        assert!(s.contains("ls-hill-far") && s.contains("ls-hill-near"));
        assert!(!s.contains("ls-sun") && !s.contains("ls-arc"));
    }

    #[test]
    fn the_sun_sits_where_the_count_says() {
        let s = svg(
            800.0,
            Sky::Slim {
                height: HEIGHT_DESKTOP,
                night: false,
            },
            Landscape::progress(3, 12),
            12,
        );
        // The desktop app's reference point: 800 wide, 3 of 12 done puts the sun at x = 214.
        assert!(s.contains(r#"class="ls-sun" cx="214""#), "{s}");
        assert!(!s.contains("style="), "the CSP forbids inline styles");
        assert!(!s.contains('#'), "no colours in the markup: {s}");
    }

    #[test]
    fn night_adds_stars_and_the_finished_scene_its_moon_and_rings() {
        let night = svg(
            600.0,
            Sky::Slim {
                height: HEIGHT_DESKTOP,
                night: true,
            },
            0.5,
            4,
        );
        assert!(night.contains("ls-horizon") && night.contains("ls-star"));
        let dusk = svg(600.0, Sky::Finished, 1.0, 4);
        assert!(dusk.contains("ls-dusk") && dusk.contains("ls-ring"));
        assert!(!dusk.contains("ls-arc"));
    }
}
