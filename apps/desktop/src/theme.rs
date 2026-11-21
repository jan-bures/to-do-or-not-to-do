//! Every visual value the desktop app uses: colours, type, spacing and radii.
//!
//! This is a stand-in for the shared `todo-design` tokens crate, which is being written in parallel. Its
//! surface mirrors that crate's (`Theme`, `palette(theme, room_hue)`, list colours, type, spacing and
//! radii), so switching over is a change to this one file: re-export from `todo_design` and delete the
//! tables. Nothing else in the app may hard-code a colour, a font family or a size.
//!
//! The hex values are the F2 "Playful" design's, computed from its OKLCH specs (room tokens use the room
//! hue, minus 10 in dark; list tokens use the list hue, minus 6 in dark). Only the room and list hues the
//! shell and its fixtures use are tabulated here.

use gpui::{Hsla, rgb};

/// Light or dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

impl Theme {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

/// OKLCH hues of the rooms and lists (light-theme hue; the dark tables already apply the shift).
pub mod hue {
    pub const MY_DAY: u16 = 65;
    pub const IMPORTANT: u16 = 28;
    pub const PLANNED: u16 = 150;
    pub const ALL: u16 = 95;
    pub const COMPLETED: u16 = 135;
    pub const AGENT: u16 = 262;
    pub const WORK: u16 = 200;
    pub const PROJECTS: u16 = 135;
    pub const PERSONAL: u16 = 10;
    pub const PLANNING: u16 = 212;
    pub const FINANCE: u16 = 182;
    pub const OFFICE: u16 = 252;
    pub const EVENTS: u16 = 158;
    pub const TODO_APP: u16 = 142;
    pub const HOME: u16 = 112;
    pub const READING: u16 = 165;
    pub const ERRANDS: u16 = 24;
    pub const HEALTH: u16 = 352;
    pub const TRAVEL: u16 = 42;
    pub const ADMIN: u16 = 330;
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Colour with alpha, for the few translucent marks (the sun's halo, stars).
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// The room palette: surfaces, ink, accent and the header landscape, all in the room's hue.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
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
    pub accent_strong: Hsla,
    #[expect(
        dead_code,
        reason = "F2 token; the filled buttons land with the detail pane"
    )]
    pub accent_fill: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub danger_bg: Hsla,
    pub danger_ink: Hsla,
    #[expect(dead_code, reason = "F2 token; the undo toast lands with sync")]
    pub toast: Hsla,
    #[expect(dead_code, reason = "F2 token; the undo toast lands with sync")]
    pub toast_ink: Hsla,
    #[expect(dead_code, reason = "F2 token; the undo toast lands with sync")]
    pub toast_ink_2: Hsla,
    #[expect(dead_code, reason = "F2 token; the undo toast lands with sync")]
    pub toast_accent: Hsla,
    pub success: Hsla,
    /// The header sky.
    pub sky: Hsla,
    pub hill_far: Hsla,
    pub hill_near: Hsla,
    /// The remaining part of the sun's arc (3:1 against the sky).
    pub arc: Hsla,
    /// Dark only: the muted band under the hills.
    pub horizon: Option<Hsla>,
    /// The sun: the only amber on screen, fixed hue.
    pub sun: Hsla,
    /// The travelled part of the arc.
    pub sun_ink: Hsla,
    /// Dark only: stars over the hills.
    pub star: Option<Hsla>,
}

struct RoomHex {
    canvas: u32,
    pane: u32,
    raised: u32,
    hover: u32,
    wash: u32,
    tint: u32,
    line: u32,
    line_strong: u32,
    control: u32,
    ink: u32,
    ink_2: u32,
    ink_3: u32,
    ink_4: u32,
    accent: u32,
    accent_strong: u32,
    accent_fill: u32,
    on_accent: u32,
    toast: u32,
    toast_ink: u32,
    toast_ink_2: u32,
    toast_accent: u32,
    sky: u32,
    hill_far: u32,
    hill_near: u32,
    arc: u32,
    horizon: u32,
    star: u32,
}

/// Room tokens, light theme. Keyed by the room hue.
fn room_light(hue: u16) -> RoomHex {
    match hue {
        hue::WORK => RoomHex {
            canvas: 0xebf5f5,
            pane: 0xf9fdfd,
            raised: 0xfdffff,
            hover: 0xeff8f8,
            wash: 0xdbf5f7,
            tint: 0xc6f2f4,
            line: 0xd9e4e5,
            line_strong: 0xc5d4d4,
            control: 0x688788,
            ink: 0x142121,
            ink_2: 0x3b4b4c,
            ink_3: 0x566768,
            ink_4: 0x899797,
            accent: 0x00a6ad,
            accent_strong: 0x00686c,
            accent_fill: 0x007e84,
            on_accent: 0xffffff,
            toast: 0x1a2526,
            toast_ink: 0xeff5f5,
            toast_ink_2: 0xb0b9ba,
            toast_accent: 0x43d5dc,
            sky: 0xdcf9fa,
            hill_far: 0xb8eef0,
            hill_near: 0x8ee2e7,
            arc: 0x689698,
            horizon: 0xd0f7f9,
            star: 0xfcfcfc,
        },
        hue::READING => RoomHex {
            canvas: 0xedf5f1,
            pane: 0xf9fdfb,
            raised: 0xfdfffe,
            hover: 0xf0f8f4,
            wash: 0xdff6ea,
            tint: 0xcdf2e0,
            line: 0xdbe5e0,
            line_strong: 0xc7d4ce,
            control: 0x6d877b,
            ink: 0x16211c,
            ink_2: 0x3e4b45,
            ink_3: 0x596760,
            ink_4: 0x8b9791,
            accent: 0x00ac7d,
            accent_strong: 0x006c4d,
            accent_fill: 0x00835e,
            on_accent: 0xffffff,
            toast: 0x1c2521,
            toast_ink: 0xf0f5f2,
            toast_ink_2: 0xb2b9b6,
            toast_accent: 0x69d6aa,
            sky: 0xe1f9ed,
            hill_far: 0xc1eed9,
            hill_near: 0x9ee3c4,
            arc: 0x709685,
            horizon: 0xd7f8e8,
            star: 0xfcfcfc,
        },
        hue::PERSONAL => RoomHex {
            canvas: 0xf9f0f1,
            pane: 0xfefbfb,
            raised: 0xfffefe,
            hover: 0xfcf3f4,
            wash: 0xffe9eb,
            tint: 0xffdfe2,
            line: 0xeadfe0,
            line_strong: 0xdaccce,
            control: 0x94787b,
            ink: 0x261b1c,
            ink_2: 0x524345,
            ink_3: 0x6e5f61,
            ink_4: 0x9d9091,
            accent: 0xd96a7e,
            accent_strong: 0x8d3d4d,
            accent_fill: 0xaa4b5e,
            on_accent: 0xffffff,
            toast: 0x2a2021,
            toast_ink: 0xf7f2f2,
            toast_ink_2: 0xbeb5b6,
            toast_accent: 0xff9dac,
            sky: 0xffedef,
            hill_far: 0xffd6db,
            hill_near: 0xffbec6,
            arc: 0xa88085,
            horizon: 0xffe8ea,
            star: 0xfcfcfc,
        },
        // My Day (h 65) and the fallback for hues not tabulated yet.
        _ => RoomHex {
            canvas: 0xf7f1ec,
            pane: 0xfefbf9,
            raised: 0xfffefd,
            hover: 0xfbf5ef,
            wash: 0xfdecdc,
            tint: 0xfee2c9,
            line: 0xe8e0da,
            line_strong: 0xd8cec6,
            control: 0x8f7c6b,
            ink: 0x241c15,
            ink_2: 0x4f463d,
            ink_3: 0x6c6158,
            ink_4: 0x9b928a,
            accent: 0xcc7d1b,
            accent_strong: 0x824c00,
            accent_fill: 0x9e5d00,
            on_accent: 0xffffff,
            toast: 0x28211b,
            toast_ink: 0xf6f3ef,
            toast_ink_2: 0xbcb6b1,
            toast_accent: 0xf3ad66,
            sky: 0xffefe1,
            hill_far: 0xfcdbbc,
            hill_near: 0xf8c799,
            arc: 0xa2866c,
            horizon: 0xffead8,
            star: 0xfcfcfc,
        },
    }
}

/// Room tokens, dark theme. Keyed by the (light) room hue; the values already use hue − 10.
fn room_dark(hue: u16) -> RoomHex {
    match hue {
        hue::WORK => RoomHex {
            canvas: 0x071110,
            pane: 0x0e1918,
            raised: 0x162221,
            hover: 0x131e1e,
            wash: 0x112928,
            tint: 0x0b3533,
            line: 0x222c2c,
            line_strong: 0x303e3d,
            control: 0x6c8684,
            ink: 0xe7efee,
            ink_2: 0xb9c3c2,
            ink_3: 0x919f9d,
            ink_4: 0x626f6e,
            accent: 0x4ac0b9,
            accent_strong: 0x7ddad4,
            accent_fill: 0x4ac0b9,
            on_accent: 0x001b1a,
            toast: 0x253432,
            toast_ink: 0xeff5f4,
            toast_ink_2: 0xb1b9b9,
            toast_accent: 0x74e0d9,
            sky: 0x0c2220,
            hill_far: 0x091d1b,
            hill_near: 0x061413,
            arc: 0x517471,
            horizon: 0x1b3735,
            star: 0xc9dcda,
        },
        hue::READING => RoomHex {
            canvas: 0x09110c,
            pane: 0x111913,
            raised: 0x19221c,
            hover: 0x161e19,
            wash: 0x18291e,
            tint: 0x1a3424,
            line: 0x252c27,
            line_strong: 0x333d36,
            control: 0x738678,
            ink: 0xe9eeea,
            ink_2: 0xbbc3bd,
            ink_3: 0x949e97,
            ink_4: 0x656f68,
            accent: 0x72bf8e,
            accent_strong: 0x98d9ae,
            accent_fill: 0x72bf8e,
            on_accent: 0x051b0e,
            toast: 0x28332c,
            toast_ink: 0xf0f5f2,
            toast_ink_2: 0xb3b9b5,
            toast_accent: 0x95deae,
            sky: 0x122118,
            hill_far: 0x0f1c13,
            hill_near: 0x0a140d,
            arc: 0x5a7362,
            horizon: 0x23372a,
            star: 0xcedcd2,
        },
        hue::PERSONAL => RoomHex {
            canvas: 0x150c0f,
            pane: 0x1c1416,
            raised: 0x261c1f,
            hover: 0x23191c,
            wash: 0x311e24,
            tint: 0x40242d,
            line: 0x30272a,
            line_strong: 0x43373a,
            control: 0x90797f,
            ink: 0xf2ebec,
            ink_2: 0xc8bec0,
            ink_3: 0xa4979b,
            ink_4: 0x74686b,
            accent: 0xe18ea8,
            accent_strong: 0xf9afc5,
            accent_fill: 0xe18ea8,
            on_accent: 0x240e15,
            toast: 0x392c30,
            toast_ink: 0xf7f2f3,
            toast_ink_2: 0xbdb5b7,
            toast_accent: 0xffb1c8,
            sky: 0x28181d,
            hill_far: 0x231418,
            hill_near: 0x190d11,
            arc: 0x80626b,
            horizon: 0x412a31,
            star: 0xe4d2d7,
        },
        _ => RoomHex {
            canvas: 0x150d08,
            pane: 0x1c1510,
            raised: 0x251d18,
            hover: 0x221a15,
            wash: 0x302116,
            tint: 0x3f2717,
            line: 0x302824,
            line_strong: 0x423832,
            control: 0x8f7c6f,
            ink: 0xf1ebe8,
            ink_2: 0xc7bfba,
            ink_3: 0xa49992,
            ink_4: 0x736a63,
            accent: 0xde9969,
            accent_strong: 0xf6b88f,
            accent_fill: 0xde9969,
            on_accent: 0x241104,
            toast: 0x392e27,
            toast_ink: 0xf7f2f0,
            toast_ink_2: 0xbdb6b1,
            toast_accent: 0xfeb98b,
            sky: 0x281a10,
            hill_far: 0x22160d,
            hill_near: 0x180f09,
            arc: 0x7f6656,
            horizon: 0x402d20,
            star: 0xe3d4cb,
        },
    }
}

/// The palette for a room: `room_hue` is one of [`hue`]'s room hues. Untabulated hues fall back to My Day.
pub fn palette(theme: Theme, room_hue: u16) -> Palette {
    let (h, dark) = match theme {
        Theme::Light => (room_light(room_hue), false),
        Theme::Dark => (room_dark(room_hue), true),
    };
    let (danger, danger_bg, danger_ink, success, sun) = if dark {
        (0xf47c6e, 0x4c221f, 0xffbfb6, 0x69c27e, 0xf8b14e)
    } else {
        (0xba3630, 0xffe7e6, 0x9a2925, 0x47aa62, 0xf5ae39)
    };
    Palette {
        canvas: c(h.canvas),
        pane: c(h.pane),
        raised: c(h.raised),
        hover: c(h.hover),
        wash: c(h.wash),
        tint: c(h.tint),
        line: c(h.line),
        line_strong: c(h.line_strong),
        control: c(h.control),
        ink: c(h.ink),
        ink_2: c(h.ink_2),
        ink_3: c(h.ink_3),
        ink_4: c(h.ink_4),
        accent: c(h.accent),
        accent_strong: c(h.accent_strong),
        accent_fill: c(h.accent_fill),
        on_accent: c(h.on_accent),
        danger: c(danger),
        danger_bg: c(danger_bg),
        danger_ink: c(danger_ink),
        toast: c(h.toast),
        toast_ink: c(h.toast_ink),
        toast_ink_2: c(h.toast_ink_2),
        toast_accent: c(h.toast_accent),
        success: c(success),
        sky: c(h.sky),
        hill_far: c(h.hill_far),
        hill_near: c(h.hill_near),
        arc: c(h.arc),
        horizon: dark.then(|| c(h.horizon)),
        // Light: the travelled arc is a darker amber ink; dark: the sun colour itself.
        sun: c(sun),
        sun_ink: c(if dark { sun } else { 0xb0660c }),
        star: dark.then(|| c(h.star)),
    }
}

/// Colours of one list: its chip, dot and sidebar glyph.
#[derive(Clone, Copy, Debug)]
pub struct ListColors {
    pub tint: Hsla,
    pub ink: Hsla,
    pub dot: Hsla,
    pub glyph: Hsla,
}

/// List colours for `list_hue` (one of [`hue`]'s list hues). Untabulated hues fall back to Planning.
pub fn list_colors(theme: Theme, list_hue: u16) -> ListColors {
    // (tint, ink, dot, glyph)
    let (tint, ink, dot, glyph) = match (theme, list_hue) {
        (Theme::Light, hue::FINANCE) => (0xccf3eb, 0x00574d, 0x00a291, 0x008173),
        (Theme::Light, hue::OFFICE) => (0xdbecff, 0x1a4c7e, 0x3d8fe4, 0x2c71b7),
        (Theme::Light, hue::EVENTS) => (0xd3f3df, 0x005936, 0x00a668, 0x008451),
        (Theme::Light, hue::TODO_APP) => (0xdaf1d7, 0x265721, 0x50a248, 0x3c8136),
        (Theme::Light, hue::HOME) => (0xeaedcd, 0x4c4f00, 0x8f9400, 0x717500),
        (Theme::Light, hue::READING) => (0xd1f3e3, 0x00593e, 0x00a577, 0x00835e),
        (Theme::Light, hue::ERRANDS) => (0xffe2df, 0x77312f, 0xd8625e, 0xad4b48),
        (Theme::Light, hue::HEALTH) => (0xffe0ec, 0x723050, 0xce6196, 0xa54a77),
        (Theme::Light, hue::TRAVEL) => (0xffe3d8, 0x763419, 0xd5683b, 0xab502a),
        (Theme::Light, hue::ADMIN) => (0xfbe0f7, 0x683464, 0xbe67b7, 0x984f92),
        (Theme::Light, _) => (0xcbf2fa, 0x005461, 0x009db3, 0x007d8e),
        (Theme::Dark, hue::FINANCE) => (0x09362d, 0xa2e8d6, 0x48caae, 0x6accb5),
        (Theme::Dark, hue::OFFICE) => (0x173045, 0xb6dcff, 0x6db8f9, 0x81bef4),
        (Theme::Dark, hue::EVENTS) => (0x193521, 0xb3e6bf, 0x72c78b, 0x85ca98),
        (Theme::Dark, hue::TODO_APP) => (0x22331a, 0xc2e3b2, 0x8ec374, 0x9ac686),
        (Theme::Dark, hue::HOME) => (0x312f0e, 0xdddba2, 0xbbb654, 0xc0bc6e),
        (Theme::Dark, hue::READING) => (0x143525, 0xade7c6, 0x66c895, 0x7dcba0),
        (Theme::Dark, hue::ERRANDS) => (0x442324, 0xffc7c8, 0xf49194, 0xf19d9f),
        (Theme::Dark, hue::HEALTH) => (0x402333, 0xfdc4e2, 0xe891c1, 0xe69dc5),
        (Theme::Dark, hue::TRAVEL) => (0x43241c, 0xffc9ba, 0xf3957b, 0xf0a08b),
        (Theme::Dark, hue::ADMIN) => (0x3a253c, 0xf1c7f4, 0xd796dc, 0xd7a2dc),
        (Theme::Dark, _) => (0x02353a, 0x9be6f0, 0x32c6d6, 0x5ecad7),
    };
    ListColors {
        tint: c(tint),
        ink: c(ink),
        dot: c(dot),
        glyph: c(glyph),
    }
}

/// Smart lists and collections draw their glyph in their own hue too (the list-token glyph, .54 .13 h).
pub fn glyph_color(theme: Theme, hue: u16) -> Hsla {
    let hex = match (theme, hue) {
        (Theme::Light, self::hue::MY_DAY) => 0x9e5d00,
        (Theme::Light, self::hue::IMPORTANT) => 0xad4c42,
        (Theme::Light, self::hue::PLANNED) => 0x258343,
        (Theme::Light, self::hue::ALL) => 0x836d00,
        (Theme::Light, self::hue::COMPLETED) => 0x4b7f28,
        (Theme::Light, self::hue::AGENT) => 0x436cb9,
        (Theme::Light, self::hue::WORK) => 0x007e84,
        (Theme::Light, self::hue::PERSONAL) => 0xac495d,
        (Theme::Dark, self::hue::MY_DAY) => 0xe7a875,
        (Theme::Dark, self::hue::IMPORTANT) => 0xf19e9b,
        (Theme::Dark, self::hue::PLANNED) => 0x90c88f,
        (Theme::Dark, self::hue::ALL) => 0xd1b46a,
        (Theme::Dark, self::hue::COMPLETED) => 0xa4c47f,
        (Theme::Dark, self::hue::AGENT) => 0x8dbaf7,
        (Theme::Dark, self::hue::WORK) => 0x5eccca,
        (Theme::Dark, self::hue::PERSONAL) => 0xee9cb0,
        (theme, h) => return list_colors(theme, h).glyph,
    };
    c(hex)
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
}

/// Letter-mark colours: (background, letter, ring).
pub fn agent_colors(theme: Theme, agent: Agent) -> (Hsla, Hsla, Hsla) {
    let (bg, ink, ring) = match (theme, agent) {
        (Theme::Light, Agent::Claude) => (0xffe2d9, 0x812f13, 0xe6af9d),
        (Theme::Light, Agent::Codex) => (0xd8edff, 0x005182, 0x98c3e8),
        (Theme::Light, Agent::Pi) => (0xdaf1d5, 0x225c13, 0xa6c99f),
        (Theme::Dark, Agent::Claude) => (0x4b281e, 0xffc9b9, 0x7a4a3b),
        (Theme::Dark, Agent::Codex) => (0x18354d, 0xb5ddff, 0x345c7d),
        (Theme::Dark, Agent::Pi) => (0x233a1e, 0xb8e7ae, 0x43623c),
    };
    (c(bg), c(ink), c(ring))
}

/// Font families. All three are OFL with latin-ext, loaded from files (see `fonts.rs`).
pub mod font {
    /// Room titles and the italic weekday eyebrow.
    pub const DISPLAY: &str = "Fraunces";
    /// All UI text.
    pub const UI: &str = "Schibsted Grotesk";
    /// Counts, due times, tags and keycaps.
    pub const MONO: &str = "Geist Mono";
}

/// Type scale: (size, line height) in logical pixels.
pub mod text {
    pub const ROOM_TITLE: (f32, f32) = (56.0, 60.0);
    pub const EYEBROW_DOW: (f32, f32) = (18.0, 22.0);
    pub const EYEBROW_MONO: (f32, f32) = (11.0, 22.0);
    pub const BODY: (f32, f32) = (14.0, 20.0);
    pub const TASK_TITLE: (f32, f32) = (15.0, 20.0);
    pub const NAV: (f32, f32) = (14.0, 20.0);
    pub const SUB_NAV: (f32, f32) = (13.5, 20.0);
    pub const COUNT: (f32, f32) = (11.5, 16.0);
    pub const LABEL: (f32, f32) = (10.5, 16.0);
    pub const SUMMARY: (f32, f32) = (12.5, 16.0);
    pub const QUICK_ADD: (f32, f32) = (14.5, 20.0);
    pub const DETAIL_TITLE: (f32, f32) = (21.0, 28.0);
    pub const TITLEBAR: (f32, f32) = (13.0, 16.0);
}

/// Weights used with [`font`].
pub mod weight {
    pub const DISPLAY: f32 = 360.0;
    pub const REGULAR: f32 = 400.0;
    pub const MEDIUM: f32 = 500.0;
    pub const SEMIBOLD: f32 = 600.0;
    pub const BOLD: f32 = 700.0;
}

/// Layout on the 4 px grid.
pub mod space {
    pub const TITLEBAR_H: f32 = 36.0;
    pub const STATUS_H: f32 = 28.0;
    pub const SIDEBAR_W: f32 = 248.0;
    pub const DETAIL_W: f32 = 392.0;
    pub const PANE_PAD_X: f32 = 32.0;
    pub const ROW_H: f32 = 56.0;
    pub const NAV_H: f32 = 32.0;
    pub const SUB_NAV_H: f32 = 28.0;
    /// Height of the hills strip under the header text.
    pub const LAND_H: f32 = 48.0;
    pub const QUICK_ADD_H: f32 = 44.0;
    pub const CHECKBOX: f32 = 20.0;
    /// Fixed row columns: agent, priority, due.
    pub const COL_AGENT: f32 = 20.0;
    pub const COL_PRIORITY: f32 = 24.0;
    pub const COL_DUE: f32 = 104.0;
    pub const COL_GAP: f32 = 12.0;
}

/// Corner radii.
pub mod radius {
    pub const SM: f32 = 4.0;
    /// Buttons, suggestion chip.
    pub const MD: f32 = 6.0;
    /// Quick-add, toast.
    pub const LG: f32 = 10.0;
    /// List chips (the only pills on desktop).
    pub const CHIP: f32 = 10.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_names() {
        assert_eq!(Theme::parse("light"), Some(Theme::Light));
        assert_eq!(Theme::parse("dark"), Some(Theme::Dark));
        assert_eq!(Theme::parse("Dark"), None);
    }

    fn hex_of(color: Hsla) -> u32 {
        let rgba = gpui::Rgba::from(color);
        let ch = |v: f32| (v * 255.0).round() as u32;
        (ch(rgba.r) << 16) | (ch(rgba.g) << 8) | ch(rgba.b)
    }

    #[test]
    fn f2_header_values_match_the_notes() {
        // The F2 NOTES table: light My Day and dark Work.
        let light = palette(Theme::Light, hue::MY_DAY);
        assert_eq!(hex_of(light.sky), 0xffefe1);
        assert_eq!(hex_of(light.hill_far), 0xfcdbbc);
        assert_eq!(hex_of(light.hill_near), 0xf8c799);
        assert_eq!(hex_of(light.arc), 0xa2866c);
        assert_eq!(hex_of(light.sun), 0xf5ae39);
        assert_eq!(hex_of(light.sun_ink), 0xb0660c);
        assert!(light.horizon.is_none() && light.star.is_none());
        let dark = palette(Theme::Dark, hue::WORK);
        assert_eq!(hex_of(dark.sky), 0x0c2220);
        assert_eq!(hex_of(dark.hill_far), 0x091d1b);
        assert_eq!(hex_of(dark.hill_near), 0x061413);
        assert_eq!(hex_of(dark.arc), 0x517471);
        assert_eq!(hex_of(dark.sun), 0xf8b14e);
        assert_eq!(dark.horizon.map(hex_of), Some(0x1b3735));
    }
}
