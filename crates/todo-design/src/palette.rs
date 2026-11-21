//! Semantic colours. Every colour is authored in OKLCH and derived from a hue, so a whole room (My Day, a
//! collection) can be re-tinted by changing one number.
//!
//! - **Room tokens** ([`Palette`]) take the room's hue. Dark mode uses hue − 10.
//! - **List tokens** ([`ListColors`]) take the list's hue. Dark mode uses hue − 6.
//! - **Agent tokens** ([`AgentColors`]) take the agent's hue, unchanged in dark mode.
//! - **Fixed tokens** (danger, success, the sun, the dusk sky) ignore the hue.
//!
//! The tables below are the single source of truth: the Rust API, `tokens.css` and `tokens.json` are all
//! generated from them.

use crate::color::{Oklch, Rgb, oklch};

/// Light or dark appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Light appearance.
    Light,
    /// Dark appearance.
    Dark,
}

impl Theme {
    /// Both themes, light first.
    pub const ALL: [Theme; 2] = [Theme::Light, Theme::Dark];

    /// `"light"` or `"dark"`.
    pub const fn key(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// How a token gets its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spec {
    /// Lightness and chroma; the hue comes from the room, list or agent. Room chroma is also scaled by the
    /// palette's chroma factor.
    Hued {
        /// OKLCH lightness.
        l: f64,
        /// OKLCH chroma.
        c: f64,
    },
    /// A fixed OKLCH colour that ignores the hue.
    Fixed(Oklch),
    /// A fixed sRGB colour.
    Srgb(Rgb),
}

const fn h(l: f64, c: f64) -> Spec {
    Spec::Hued { l, c }
}

const fn fixed(l: f64, c: f64, hue: f64) -> Spec {
    Spec::Fixed(Oklch::new(l, c, hue))
}

impl Spec {
    /// Resolves the spec for a hue (already shifted for the theme) and a chroma factor.
    pub fn resolve(self, hue: f64, chroma_scale: f64) -> Rgb {
        match self {
            Spec::Hued { l, c } => oklch(l, c * chroma_scale, hue),
            Spec::Fixed(o) => o.to_rgb(),
            Spec::Srgb(rgb) => rgb,
        }
    }
}

macro_rules! token_struct {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[doc = $doc:literal])* $field:ident = $light:expr, $dark:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            $( $(#[doc = $doc])* pub $field: Rgb, )*
        }

        impl $name {
            /// Token names in kebab case (as in CSS), in declaration order.
            pub const NAMES: &'static [&'static str] = &[$( kebab(stringify!($field)) ),*];

            /// `(kebab-case name, colour)` for every token, in declaration order.
            pub fn entries(&self) -> Vec<(&'static str, Rgb)> {
                let values = [$( self.$field ),*];
                Self::NAMES.iter().copied().zip(values).collect()
            }

            fn from_specs(theme: Theme, hue: f64, chroma_scale: f64) -> $name {
                let pick = |light: Spec, dark: Spec| match theme {
                    Theme::Light => light.resolve(hue, chroma_scale),
                    Theme::Dark => dark.resolve(hue, chroma_scale),
                };
                $name { $( $field: pick($light, $dark), )* }
            }

            /// The token called `name` (kebab case).
            pub fn get(&self, name: &str) -> Option<Rgb> {
                $( if name == kebab(stringify!($field)) { return Some(self.$field); } )*
                None
            }

            /// Replaces the token called `name` (kebab case). Returns false if there is no such token.
            #[allow(dead_code, reason = "only the room palette has contrast guards")]
            fn set(&mut self, name: &str, value: Rgb) -> bool {
                $( if name == kebab(stringify!($field)) { self.$field = value; return true; } )*
                false
            }

            /// The OKLCH specs behind the tokens: `(name, light, dark)`.
            pub fn specs() -> Vec<(&'static str, Spec, Spec)> {
                let specs = [$( ($light, $dark) ),*];
                Self::NAMES.iter().copied().zip(specs).map(|(n, (l, d))| (n, l, d)).collect()
            }
        }
    };
}

/// Turns a snake_case field name into kebab case at compile time.
const fn kebab(s: &'static str) -> &'static str {
    // Field names are short ASCII; the kebab form is looked up from a fixed table so it can stay `const`.
    match s.as_bytes() {
        b"line_strong" => "line-strong",
        b"ink_2" => "ink-2",
        b"ink_3" => "ink-3",
        b"ink_4" => "ink-4",
        b"accent_strong" => "accent-strong",
        b"accent_fill" => "accent-fill",
        b"on_accent" => "on-accent",
        b"danger_bg" => "danger-bg",
        b"danger_ink" => "danger-ink",
        b"toast_ink" => "toast-ink",
        b"toast_ink_2" => "toast-ink-2",
        b"toast_accent" => "toast-accent",
        b"hill_far" => "hill-far",
        b"hill_near" => "hill-near",
        b"sun_ink" => "sun-ink",
        b"on_strong" => "on-strong",
        b"silhouette_far" => "silhouette-far",
        b"silhouette_near" => "silhouette-near",
        _ => s,
    }
}

token_struct! {
    /// The colours of one room (My Day, a collection, a list) in one theme. Get one from [`palette()`].
    Palette {
        /// Window background, sidebar, title bar, status bar.
        canvas = h(0.962, 0.010), h(0.168, 0.016);
        /// The task list and detail pane.
        pane = h(0.990, 0.004), h(0.202, 0.015);
        /// Inputs, the selected sidebar item, secondary buttons.
        raised = h(0.998, 0.002), h(0.240, 0.016);
        /// Hover background.
        hover = h(0.972, 0.010), h(0.226, 0.016);
        /// The selected row, the active step, the quick-add parse strip.
        wash = h(0.952, 0.028), h(0.262, 0.030);
        /// Highlights for parsed quick-add text, the focus ring, the sunrise mark.
        tint = h(0.930, 0.045), h(0.300, 0.045);
        /// Hairlines between rows and panes.
        line = h(0.912, 0.012), h(0.285, 0.014);
        /// Keycap borders, secondary button rings, separators that must read.
        line_strong = h(0.858, 0.016), h(0.350, 0.018);
        /// Control boundaries: checkbox rings and input borders (3:1 on pane).
        control = h(0.600, 0.035), h(0.600, 0.030);
        /// Primary text.
        ink = h(0.235, 0.018), h(0.945, 0.008);
        /// Secondary text.
        ink_2 = h(0.400, 0.020), h(0.810, 0.012);
        /// Metadata: counts, tags, timestamps (4.5:1 on pane, canvas and wash).
        ink_3 = h(0.500, 0.020), h(0.690, 0.016);
        /// Disabled text and decorative separators. Never for information.
        ink_4 = h(0.665, 0.016), h(0.530, 0.016);
        /// The checked checkbox, the selection bar, meters.
        accent = h(0.660, 0.140), h(0.740, 0.105);
        /// Accent text: "due soon", the estimate, the quick-add plus.
        accent_strong = h(0.470, 0.110), h(0.830, 0.090);
        /// Primary button fill.
        accent_fill = h(0.540, 0.125), h(0.740, 0.105);
        /// Label and tick on `accent_fill` and on the checked checkbox.
        on_accent = Spec::Srgb(Rgb::WHITE), h(0.200, 0.040);
        /// Overdue checkbox ring.
        danger = fixed(0.530, 0.170, 27.0), fixed(0.720, 0.150, 28.0);
        /// Overdue badge background and overdue checkbox fill.
        danger_bg = fixed(0.948, 0.032, 22.0), fixed(0.310, 0.065, 25.0);
        /// Overdue badge text.
        danger_ink = fixed(0.460, 0.150, 27.0), fixed(0.860, 0.090, 28.0);
        /// Toast and tooltip background.
        toast = h(0.255, 0.016), h(0.310, 0.020);
        /// Toast text.
        toast_ink = h(0.965, 0.006), h(0.965, 0.006);
        /// Toast secondary text ("just now"), the countdown line.
        toast_ink_2 = h(0.780, 0.010), h(0.780, 0.010);
        /// The toast's Undo action.
        toast_accent = h(0.800, 0.120), h(0.840, 0.100);
        /// The "synced" dot.
        success = fixed(0.660, 0.140, 150.0), fixed(0.740, 0.130, 150.0);
        /// The header sky behind the room title.
        sky = h(0.962, 0.030), h(0.232, 0.028);
        /// The far hill.
        hill_far = h(0.912, 0.055), h(0.212, 0.026);
        /// The near hill.
        hill_near = h(0.862, 0.082), h(0.178, 0.020);
        /// The sun's remaining path (3:1 on sky).
        arc = h(0.640, 0.050), h(0.530, 0.040);
        /// The band under the hills. Drawn in dark only.
        horizon = h(0.950, 0.040), h(0.315, 0.035);
        /// The sun. The only amber on screen, at a fixed hue.
        sun = fixed(0.800, 0.150, 75.0), fixed(0.810, 0.140, 72.0);
        /// The sun's travelled path.
        sun_ink = fixed(0.580, 0.130, 62.0), fixed(0.810, 0.140, 72.0);
        /// Stars (drawn in dark only) and the moon.
        star = h(0.990, 0.000), h(0.880, 0.020);
    }
}

token_struct! {
    /// The colours of one list hue: the chip, the dot, the sidebar glyph and the active room pill. Get one from
    /// [`list_colors()`].
    ListColors {
        /// Chip background.
        tint = h(0.935, 0.042), h(0.300, 0.050);
        /// Chip text.
        ink = h(0.410, 0.100), h(0.880, 0.075);
        /// The 6 px chip dot and the room-pill dot.
        dot = h(0.640, 0.150), h(0.760, 0.120);
        /// The hand-drawn list glyph in the sidebar.
        glyph = h(0.540, 0.130), h(0.780, 0.100);
        /// The active room pill (phone).
        strong = h(0.700, 0.125), h(0.720, 0.105);
        /// Text on `strong`: white or a deep shade of the hue, whichever contrasts more. Derived, not specified.
        on_strong = Spec::Srgb(Rgb::WHITE), Spec::Srgb(Rgb::WHITE);
    }
}

token_struct! {
    /// The colours of an agent's letter mark (C, X, π). Get one from [`agent_colors()`].
    AgentColors {
        /// Mark background.
        bg = h(0.935, 0.045), h(0.320, 0.055);
        /// The letter.
        ink = h(0.420, 0.120), h(0.880, 0.090);
        /// The 1 px inset ring.
        ring = h(0.800, 0.070), h(0.460, 0.070);
    }
}

token_struct! {
    /// The finished-day scene: dusk sky, silhouettes, stars and the setting sun. It is a night scene in both
    /// themes, so it is derived from the dark specs (room hue − 10). Get one from [`dusk()`].
    Dusk {
        /// Top of the dusk gradient, and the header behind the title.
        top = fixed(0.205, 0.035, 265.0), fixed(0.205, 0.035, 265.0);
        /// Bottom of the dusk gradient, at the horizon.
        low = fixed(0.440, 0.085, 48.0), fixed(0.440, 0.085, 48.0);
        /// Far hill silhouette.
        silhouette_far = h(0.190, 0.020), h(0.190, 0.020);
        /// Near hill silhouette.
        silhouette_near = h(0.155, 0.014), h(0.155, 0.014);
        /// Stars and the moon.
        star = h(0.880, 0.020), h(0.880, 0.020);
        /// The setting sun and its rings.
        sun = fixed(0.810, 0.140, 72.0), fixed(0.810, 0.140, 72.0);
    }
}

/// Room hues shift by this much in dark mode.
pub const ROOM_DARK_HUE_SHIFT: f64 = -10.0;
/// List hues shift by this much in dark mode.
pub const LIST_DARK_HUE_SHIFT: f64 = -6.0;
/// The deep shade that [`ListColors::on_strong`] falls back to: OKLCH lightness and chroma at the list hue.
pub const LIST_ON_STRONG_DARK: (f64, f64) = (0.24, 0.05);

/// Contrast guards: `(foreground, backgrounds, minimum WCAG ratio)`.
///
/// OKLCH lightness is perceptual, but WCAG contrast is computed from luminance, and the two disagree by hue: at
/// the same lightness, greens and cyans have more luminance than reds and violets. So a pair that passes at one
/// room hue can fall just short at another. After a palette is derived, each guarded foreground whose contrast
/// against any of its backgrounds is below the minimum moves away from them in lightness, in steps of
/// [`GUARD_STEP`], until it passes. Pairs that already pass, including every pair in the approved contrast table,
/// are untouched.
pub const GUARDS: [(&str, &[&str], f64); 14] = [
    ("ink", &["pane", "canvas", "wash"], 4.5),
    ("ink-2", &["pane", "sky"], 4.5),
    ("ink-3", &["pane", "canvas", "wash"], 4.5),
    ("accent-strong", &["pane", "sky"], 4.5),
    ("danger-ink", &["danger-bg"], 4.5),
    ("toast-ink", &["toast"], 4.5),
    ("toast-ink-2", &["toast"], 4.5),
    ("toast-accent", &["toast"], 4.5),
    ("on-accent", &["accent-fill"], 4.5),
    ("accent", &["pane"], 3.0),
    ("control", &["pane", "wash"], 3.0),
    ("danger", &["pane"], 3.0),
    ("arc", &["sky"], 3.0),
    ("sun-ink", &["sky"], 3.0),
];

/// The lightness step of a contrast guard.
pub const GUARD_STEP: f64 = 0.005;

fn apply_guards(p: &mut Palette, theme: Theme, hue: f64, chroma_scale: f64) {
    let specs = Palette::specs();
    for (fg, bgs, min) in GUARDS {
        let Some(spec) = specs.iter().find(|s| s.0 == fg).map(|s| match theme {
            Theme::Light => s.1,
            Theme::Dark => s.2,
        }) else {
            continue;
        };
        let (l, c, h) = match spec {
            Spec::Hued { l, c } => (l, c * chroma_scale, hue),
            Spec::Fixed(o) => (o.l, o.c, o.h),
            Spec::Srgb(_) => continue,
        };
        let backgrounds: Vec<Rgb> = bgs.iter().filter_map(|b| p.get(b)).collect();
        let passes = |c: Rgb| backgrounds.iter().all(|b| c.contrast(*b) >= min);
        let Some(current) = p.get(fg) else { continue };
        if passes(current) || backgrounds.is_empty() {
            continue;
        }
        let lighter = current.luminance() > backgrounds[0].luminance();
        let dir = if lighter { 1.0 } else { -1.0 };
        for i in 1..=100 {
            let candidate = oklch((l + dir * GUARD_STEP * f64::from(i)).clamp(0.0, 1.0), c, h);
            if passes(candidate) {
                p.set(fg, candidate);
                break;
            }
        }
    }
}

/// The room hue as used in `theme`.
pub fn room_hue(theme: Theme, hue: f64) -> f64 {
    match theme {
        Theme::Light => hue,
        Theme::Dark => hue + ROOM_DARK_HUE_SHIFT,
    }
}

/// The list hue as used in `theme`.
pub fn list_hue(theme: Theme, hue: f64) -> f64 {
    match theme {
        Theme::Light => hue,
        Theme::Dark => hue + LIST_DARK_HUE_SHIFT,
    }
}

/// The palette for a room with hue `room_hue` (degrees, as listed in [`crate::hues`]).
pub fn palette(theme: Theme, room_hue: f64) -> Palette {
    palette_scaled(theme, room_hue, 1.0)
}

/// [`palette()`] with the room-hued chroma multiplied by `chroma_scale`. The inner phone screen uses 0.85 for
/// rooms whose hue would otherwise feel loud on a large dark surface.
pub fn palette_scaled(theme: Theme, hue: f64, chroma_scale: f64) -> Palette {
    let hh = room_hue(theme, hue);
    let mut p = Palette::from_specs(theme, hh, chroma_scale);
    apply_guards(&mut p, theme, hh, chroma_scale);
    p
}

/// The colours for a list with hue `hue`.
pub fn list_colors(theme: Theme, hue: f64) -> ListColors {
    let hh = list_hue(theme, hue);
    let mut colors = ListColors::from_specs(theme, hh, 1.0);
    let deep = oklch(LIST_ON_STRONG_DARK.0, LIST_ON_STRONG_DARK.1, hh);
    colors.on_strong = if Rgb::WHITE.contrast(colors.strong) >= deep.contrast(colors.strong) {
        Rgb::WHITE
    } else {
        deep
    };
    colors
}

/// The colours for an agent's letter mark with hue `hue`.
pub fn agent_colors(theme: Theme, hue: f64) -> AgentColors {
    AgentColors::from_specs(theme, hue, 1.0)
}

/// The finished-day scene for a room with hue `hue`. The same in both themes.
pub fn dusk(hue: f64) -> Dusk {
    Dusk::from_specs(Theme::Dark, room_hue(Theme::Dark, hue), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_kebab_case_and_unique() {
        for names in [
            Palette::NAMES,
            ListColors::NAMES,
            AgentColors::NAMES,
            Dusk::NAMES,
        ] {
            let mut seen = std::collections::BTreeSet::new();
            for n in names {
                assert!(!n.contains('_'), "{n} is not kebab case");
                assert!(seen.insert(*n), "{n} is duplicated");
            }
        }
    }

    #[test]
    fn guards_name_real_tokens() {
        for (fg, bgs, _) in GUARDS {
            assert!(Palette::NAMES.contains(&fg), "{fg}");
            for b in bgs {
                assert!(Palette::NAMES.contains(b), "{b}");
            }
        }
    }

    #[test]
    fn guards_lift_a_failing_pair_and_leave_passing_ones() {
        // Hue 140 in light: the raw accent spec is just under 3:1 on pane.
        let raw = Palette::from_specs(Theme::Light, 140.0, 1.0);
        assert!(raw.accent.contrast(raw.pane) < 3.0);
        let p = palette(Theme::Light, 140.0);
        assert!(p.accent.contrast(p.pane) >= 3.0);
        assert!(
            p.accent.contrast(p.pane) < 3.2,
            "moved only as far as needed"
        );
        // My Day passes as specified, so nothing moves.
        assert_eq!(
            palette(Theme::Light, 65.0),
            Palette::from_specs(Theme::Light, 65.0, 1.0)
        );
    }

    #[test]
    fn entries_follow_declaration_order() {
        let p = palette(Theme::Light, 65.0);
        let e = p.entries();
        assert_eq!(e[0], ("canvas", p.canvas));
        assert_eq!(e.last().copied(), Some(("star", p.star)));
        assert_eq!(e.len(), Palette::NAMES.len());
    }
}
