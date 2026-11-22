//! Windows caption button colours and desktop scrollbar opacities.

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

pub(crate) const COLORS: [(&str, Rgb); 3] = [
    ("caption-close", CAPTION_CLOSE),
    ("caption-close-pressed", CAPTION_CLOSE_PRESSED),
    ("caption-white", CAPTION_WHITE),
];

pub(crate) const OPACITIES: [(&str, f32); 6] = [
    ("caption-hover-light", CAPTION_HOVER_LIGHT),
    ("caption-pressed-light", CAPTION_PRESSED_LIGHT),
    ("caption-hover-dark", CAPTION_HOVER_DARK),
    ("caption-pressed-dark", CAPTION_PRESSED_DARK),
    ("scrollbar-alpha", SCROLLBAR_ALPHA),
    ("scrollbar-hover-alpha", SCROLLBAR_HOVER_ALPHA),
];
