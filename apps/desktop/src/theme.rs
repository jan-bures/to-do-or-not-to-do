//! The desktop app's view of the design tokens in `todo_design`: GPUI colours, text styles and the token
//! modules. Nothing else in the app hard-codes a colour, a font family, a size or a duration; it all comes
//! from here, and everything here comes from `todo_design`.

use gpui::{FontWeight, Hsla, Styled, px, rgb};
use todo_design::Rgb;
pub use todo_design::Theme;
pub use todo_design::layout::{component, desktop, elevation, radius, space};
pub use todo_design::typography as ty;
pub use todo_design::{Glyph, Icon, hues, landscape};

/// A hue in OKLCH degrees, as `todo_design` takes it.
pub type Hue = f64;

/// Parses `light` or `dark`.
pub fn parse_theme(s: &str) -> Option<Theme> {
    Theme::ALL.into_iter().find(|t| t.key() == s)
}

/// A token colour as a GPUI colour.
pub fn color(c: Rgb) -> Hsla {
    rgb(c.to_u32()).into()
}

/// The same colour at `alpha` opacity.
pub fn with_alpha(c: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..c }
}

/// The room palette in GPUI colours: the tokens the shell draws with. See `todo_design::Palette` for what
/// each one is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub theme: Theme,
    pub canvas: Hsla,
    pub pane: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub wash: Hsla,
    pub tint: Hsla,
    pub line: Hsla,
    pub line_strong: Hsla,
    pub control: Hsla,
    pub ink: Hsla,
    pub ink_2: Hsla,
    pub ink_3: Hsla,
    pub ink_4: Hsla,
    pub accent: Hsla,
    pub check_selected: Hsla,
    pub accent_strong: Hsla,
    pub accent_fill: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub danger_bg: Hsla,
    pub danger_ink: Hsla,
    pub success: Hsla,
    pub sky: Hsla,
    pub hill_far: Hsla,
    pub hill_near: Hsla,
    pub arc: Hsla,
    pub horizon: Hsla,
    pub sun: Hsla,
    pub sun_ink: Hsla,
    pub star: Hsla,
}

impl Palette {
    /// The horizon band and the stars are drawn in the dark slim scene only.
    pub fn night(&self) -> bool {
        self.theme == Theme::Dark
    }
}

/// The palette of a room with hue `room_hue`.
pub fn palette(theme: Theme, room_hue: Hue) -> Palette {
    let t = todo_design::palette(theme, room_hue);
    Palette {
        theme,
        canvas: color(t.canvas),
        pane: color(t.pane),
        raised: color(t.raised),
        hover: color(t.hover),
        wash: color(t.wash),
        tint: color(t.tint),
        line: color(t.line),
        line_strong: color(t.line_strong),
        control: color(t.control),
        ink: color(t.ink),
        ink_2: color(t.ink_2),
        ink_3: color(t.ink_3),
        ink_4: color(t.ink_4),
        accent: color(t.accent),
        check_selected: color(t.check_selected),
        accent_strong: color(t.accent_strong),
        accent_fill: color(t.accent_fill),
        on_accent: color(t.on_accent),
        danger: color(t.danger),
        danger_bg: color(t.danger_bg),
        danger_ink: color(t.danger_ink),
        success: color(t.success),
        sky: color(t.sky),
        hill_far: color(t.hill_far),
        hill_near: color(t.hill_near),
        arc: color(t.arc),
        horizon: color(t.horizon),
        sun: color(t.sun),
        sun_ink: color(t.sun_ink),
        star: color(t.star),
    }
}

/// The finished-day scene of a room, in GPUI colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dusk {
    pub top: Hsla,
    pub low: Hsla,
    pub silhouette_far: Hsla,
    pub silhouette_near: Hsla,
    pub star: Hsla,
    pub sun: Hsla,
}

pub fn dusk(room_hue: Hue) -> Dusk {
    let d = todo_design::dusk(room_hue);
    Dusk {
        top: color(d.top),
        low: color(d.low),
        silhouette_far: color(d.silhouette_far),
        silhouette_near: color(d.silhouette_near),
        star: color(d.star),
        sun: color(d.sun),
    }
}

/// Colours of one list: its chip, dot and sidebar glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListColors {
    pub tint: Hsla,
    pub ink: Hsla,
    pub dot: Hsla,
    pub glyph: Hsla,
}

pub fn list_colors(theme: Theme, hue: Hue) -> ListColors {
    let c = todo_design::list_colors(theme, hue);
    ListColors {
        tint: color(c.tint),
        ink: color(c.ink),
        dot: color(c.dot),
        glyph: color(c.glyph),
    }
}

/// An agent's letter mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Pi,
}

impl Agent {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Claude => "C",
            Self::Codex => "X",
            Self::Pi => "π",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    /// The agent's default hue from `todo_design::hues::AGENTS`.
    pub fn hue(self) -> Hue {
        hues::AGENTS
            .iter()
            .find(|h| h.key == self.key())
            .map_or(0.0, |h| h.degrees)
    }
}

/// Letter-mark colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentColors {
    pub bg: Hsla,
    pub ink: Hsla,
    pub ring: Hsla,
}

pub fn agent_colors(theme: Theme, agent: Agent) -> AgentColors {
    let c = todo_design::agent_colors(theme, agent.hue());
    AgentColors {
        bg: color(c.bg),
        ink: color(c.ink),
        ring: color(c.ring),
    }
}

/// A list swatch's hue by key (`"lagoon"`, `"rose"`, ...); My Day's hue for an unknown key. Fixtures pick
/// lists' hues this way, as the owner would in the app. `const`, so fixtures can use it in constants.
pub const fn swatch(key: &str) -> Hue {
    let mut i = 0;
    while i < hues::SWATCHES.len() {
        if str_eq(hues::SWATCHES[i].key, key) {
            return hues::SWATCHES[i].degrees;
        }
        i += 1;
    }
    hues::MY_DAY
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Applies a `todo_design` text style: family, size, line height, weight and slant. GPUI at the pinned
/// revision has no letter spacing, so `tracking_em` is left to the few places that fake it (priority marks),
/// and capitals are the caller's job (`to_uppercase`).
pub trait TextStyled: Styled + Sized {
    fn style(self, s: ty::TextStyle) -> Self {
        let el = self
            .font_family(s.family)
            .text_size(px(s.size))
            .line_height(px(s.line_height))
            .font_weight(FontWeight(f32::from(s.weight)));
        if s.italic { el.italic() } else { el }
    }
}

impl<T: Styled> TextStyled for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_names() {
        assert_eq!(parse_theme("light"), Some(Theme::Light));
        assert_eq!(parse_theme("dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("Dark"), None);
    }

    #[test]
    fn colours_come_from_the_tokens() {
        let p = palette(Theme::Light, hues::MY_DAY);
        let t = todo_design::palette(Theme::Light, hues::MY_DAY);
        assert_eq!(p.pane, color(t.pane));
        assert_eq!(p.check_selected, color(t.check_selected));
        // The guard nudges the checked box on a selected row away from plain accent in My Day.
        assert_ne!(p.check_selected, p.accent);
    }

    #[test]
    fn agents_and_swatches_resolve_to_named_hues() {
        assert_eq!(Agent::Codex.hue(), 245.0);
        assert_eq!(swatch("lagoon"), 200.0);
        assert_eq!(swatch("no-such-swatch"), hues::MY_DAY);
    }
}
