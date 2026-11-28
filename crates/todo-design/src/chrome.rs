//! Windows caption button colours, desktop scrollbar opacities and Windows acrylic material opacities.

use crate::Rgb;

/// Windows 11 close hover, specified by the OS in sRGB, not by the room hue.
pub const CAPTION_CLOSE: Rgb = Rgb::from_u32(0xc42b1c);
/// A darker red while the close button is pressed.
pub const CAPTION_CLOSE_PRESSED: Rgb = Rgb::from_u32(0xa92317);
/// Glyph ink for the red close button and the base of the dark caption tint.
pub const CAPTION_WHITE: Rgb = Rgb::from_u32(0xffffff);

/// Alpha of the caption hover tint in light mode (overlaid with room ink).
pub const CAPTION_HOVER_LIGHT: f32 = 0.06;
/// Alpha of the caption press tint in light mode (overlaid with room ink).
pub const CAPTION_PRESSED_LIGHT: f32 = 0.04;
/// Alpha of the caption hover tint in dark mode (overlaid with white).
pub const CAPTION_HOVER_DARK: f32 = 0.08;
/// Alpha of the caption press tint in dark mode (overlaid with white).
pub const CAPTION_PRESSED_DARK: f32 = 0.06;

/// Alpha of the quiet scrollbar thumb, tinted with room ink.
pub const SCROLLBAR_ALPHA: f32 = 0.26;
/// Alpha of the thumb when hovered or dragged.
pub const SCROLLBAR_HOVER_ALPHA: f32 = 0.52;

/// Alpha of the room-hued `canvas` chrome (title bar, sidebar, status bar) over Windows acrylic in light mode.
pub const ACRYLIC_CANVAS_LIGHT: f32 = 0.66;
/// Alpha of the room-hued `canvas` chrome over Windows acrylic in dark mode.
pub const ACRYLIC_CANVAS_DARK: f32 = 0.62;
/// Alpha of the room-hued `sky` header over Windows acrylic in light mode.
pub const ACRYLIC_SKY_LIGHT: f32 = 0.78;
/// Alpha of the room-hued `sky` header over Windows acrylic in dark mode.
pub const ACRYLIC_SKY_DARK: f32 = 0.76;
/// Alpha of primary reading `pane` surfaces over Windows acrylic in light mode.
pub const ACRYLIC_PANE_LIGHT: f32 = 0.88;
/// Alpha of primary reading `pane` surfaces over Windows acrylic in dark mode.
pub const ACRYLIC_PANE_DARK: f32 = 0.84;
/// Alpha of controls and hover fills sitting on the light acrylic chrome.
pub const ACRYLIC_CONTROL_LIGHT: f32 = 0.74;
/// Alpha of controls and hover fills sitting on the dark acrylic chrome.
pub const ACRYLIC_CONTROL_DARK: f32 = 0.70;
/// Alpha of the finished dusk scene over Windows acrylic, keeping the night sky deep in both themes.
pub const ACRYLIC_DUSK: f32 = 0.94;

pub(crate) const COLORS: [(&str, Rgb); 3] = [
    ("caption-close", CAPTION_CLOSE),
    ("caption-close-pressed", CAPTION_CLOSE_PRESSED),
    ("caption-white", CAPTION_WHITE),
];

pub(crate) const OPACITIES: [(&str, f32); 15] = [
    ("caption-hover-light", CAPTION_HOVER_LIGHT),
    ("caption-pressed-light", CAPTION_PRESSED_LIGHT),
    ("caption-hover-dark", CAPTION_HOVER_DARK),
    ("caption-pressed-dark", CAPTION_PRESSED_DARK),
    ("scrollbar-alpha", SCROLLBAR_ALPHA),
    ("scrollbar-hover-alpha", SCROLLBAR_HOVER_ALPHA),
    ("acrylic-canvas-light", ACRYLIC_CANVAS_LIGHT),
    ("acrylic-canvas-dark", ACRYLIC_CANVAS_DARK),
    ("acrylic-sky-light", ACRYLIC_SKY_LIGHT),
    ("acrylic-sky-dark", ACRYLIC_SKY_DARK),
    ("acrylic-pane-light", ACRYLIC_PANE_LIGHT),
    ("acrylic-pane-dark", ACRYLIC_PANE_DARK),
    ("acrylic-control-light", ACRYLIC_CONTROL_LIGHT),
    ("acrylic-control-dark", ACRYLIC_CONTROL_DARK),
    ("acrylic-dusk", ACRYLIC_DUSK),
];
