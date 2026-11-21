//! Colour math: OKLCH to sRGB with gamut mapping by chroma reduction, and WCAG 2.x contrast.
//!
//! This is a straight port of the design generator, so the hex values here are the hex values in the
//! approved mockups. Every token is authored in OKLCH and emitted as 8-bit sRGB.

use std::fmt;

/// An opaque 8-bit sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red, 0–255.
    pub r: u8,
    /// Green, 0–255.
    pub g: u8,
    /// Blue, 0–255.
    pub b: u8,
}

impl Rgb {
    /// White, used as the label colour on some fills.
    pub const WHITE: Rgb = Rgb::from_u32(0xffffff);

    /// Builds a colour from `0xRRGGBB`.
    pub const fn from_u32(hex: u32) -> Rgb {
        Rgb {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }

    /// `0xRRGGBB`, the form GPUI's `rgb()` takes.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Lower-case `#rrggbb`.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Parses `#rrggbb` or `rrggbb`.
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb::from_u32)
    }

    /// The same colour with an alpha channel.
    pub const fn with_alpha(self, alpha: f32) -> Rgba {
        Rgba { rgb: self, alpha }
    }

    /// WCAG 2.x relative luminance.
    pub fn luminance(self) -> f64 {
        let lin = |c: u8| srgb_to_linear(f64::from(c) / 255.0);
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG 2.x contrast ratio against `other`, from 1.0 to 21.0.
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// An sRGB colour with straight (not premultiplied) alpha, for the few translucent paints (sun halo, stars,
/// overlay shadows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    /// The opaque colour.
    pub rgb: Rgb,
    /// Opacity, 0.0–1.0.
    pub alpha: f32,
}

impl Rgba {
    /// CSS `rgb(r g b / a)`.
    pub fn css(self) -> String {
        format!(
            "rgb({} {} {} / {})",
            self.rgb.r, self.rgb.g, self.rgb.b, self.alpha
        )
    }
}

/// An OKLCH colour: lightness 0–1, chroma (about 0–0.37), hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    /// Perceptual lightness, 0.0 (black) to 1.0 (white).
    pub l: f64,
    /// Chroma. 0.0 is grey.
    pub c: f64,
    /// Hue angle in degrees. Any value; it wraps.
    pub h: f64,
}

impl Oklch {
    /// Builds an OKLCH colour.
    pub const fn new(l: f64, c: f64, h: f64) -> Oklch {
        Oklch { l, c, h }
    }

    /// Converts to 8-bit sRGB. Out-of-gamut colours keep their lightness and hue and lose chroma, found by a
    /// 24-step binary search, exactly as the design generator does.
    pub fn to_rgb(self) -> Rgb {
        let mut rgb = oklch_to_linear(self.l, self.c, self.h);
        if !in_gamut(rgb) {
            let (mut lo, mut hi) = (0.0, self.c);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if in_gamut(oklch_to_linear(self.l, mid, self.h)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            rgb = oklch_to_linear(self.l, lo, self.h);
        }
        // Python's round() rounds half to even; match it so the hex values agree bit for bit.
        let channel = |c: f64| {
            (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0)
                .round_ties_even()
                .clamp(0.0, 255.0) as u8
        };
        Rgb {
            r: channel(rgb[0]),
            g: channel(rgb[1]),
            b: channel(rgb[2]),
        }
    }
}

/// Shorthand for `Oklch::new(l, c, h).to_rgb()`.
pub fn oklch(l: f64, c: f64, h: f64) -> Rgb {
    Oklch::new(l, c, h).to_rgb()
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn oklch_to_linear(l: f64, c: f64, h: f64) -> [f64; 3] {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3,
    ]
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    const EPS: f64 = 1e-4;
    rgb.iter().all(|&c| (-EPS..=1.0 + EPS).contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let c = Rgb::from_hex("#f5ae39").expect("valid hex");
        assert_eq!(c, Rgb::from_u32(0xf5ae39));
        assert_eq!(c.hex(), "#f5ae39");
        assert_eq!(c.to_string(), "#f5ae39");
        assert_eq!(Rgb::from_hex("f5ae3"), None);
    }

    #[test]
    fn extremes() {
        assert_eq!(oklch(1.0, 0.0, 0.0), Rgb::WHITE);
        assert_eq!(oklch(0.0, 0.0, 0.0), Rgb::from_u32(0));
        let ratio = Rgb::WHITE.contrast(Rgb::from_u32(0));
        assert!((ratio - 21.0).abs() < 1e-9);
    }

    #[test]
    fn out_of_gamut_keeps_hue_and_loses_chroma() {
        // Chroma 0.4 is far outside sRGB at this lightness; the result must still be a valid colour close to
        // the in-gamut maximum.
        let c = oklch(0.7, 0.4, 150.0);
        assert!(c.g > c.r && c.g > c.b);
    }
}
