//! The bundled font files in `assets/fonts/`: static instances of Geist and Geist Mono
//! (all SIL OFL 1.1), subset to Latin and Latin Extended so Czech renders fully. `assets/fonts/README.md`
//! records the upstream sources and how the instances are built.
//!
//! Register every face with the platform's text system, then ask for the family names in
//! [`crate::typography`] at the weights listed here.

/// One bundled font file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFace {
    /// Family name, as stored in the file's typographic family name.
    pub family: &'static str,
    /// Weight (`usWeightClass`).
    pub weight: u16,
    /// Italic.
    pub italic: bool,
    /// File name inside `assets/fonts/`.
    pub file: &'static str,
}

impl FontFace {
    /// The path relative to the repository root.
    pub fn path(&self) -> String {
        format!("assets/fonts/{}", self.file)
    }
}

const fn face(family: &'static str, weight: u16, italic: bool, file: &'static str) -> FontFace {
    FontFace {
        family,
        weight,
        italic,
        file,
    }
}

use crate::typography::{MONO, UI};

/// Every bundled face.
pub const FACES: [FontFace; 7] = [
    face(UI, 400, false, "Geist-400.ttf"),
    face(UI, 500, false, "Geist-500.ttf"),
    face(UI, 600, false, "Geist-600.ttf"),
    face(MONO, 400, false, "GeistMono-400.ttf"),
    face(MONO, 500, false, "GeistMono-500.ttf"),
    face(MONO, 600, false, "GeistMono-600.ttf"),
    face(MONO, 700, false, "GeistMono-700.ttf"),
];

/// The face for a family, weight and slant, if it is bundled.
pub fn find(family: &str, weight: u16, italic: bool) -> Option<FontFace> {
    FACES
        .iter()
        .copied()
        .find(|f| f.family == family && f.weight == weight && f.italic == italic)
}

#[cfg(feature = "embed-fonts")]
impl FontFace {
    /// The font file's bytes, embedded at compile time.
    pub fn bytes(&self) -> &'static [u8] {
        macro_rules! embed {
            ($($file:literal),*) => {
                match self.file {
                    $( $file => include_bytes!(concat!("../../../assets/fonts/", $file)), )*
                    _ => &[],
                }
            };
        }
        embed!(
            "Geist-400.ttf",
            "Geist-500.ttf",
            "Geist-600.ttf",
            "GeistMono-400.ttf",
            "GeistMono-500.ttf",
            "GeistMono-600.ttf",
            "GeistMono-700.ttf"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typography;

    #[test]
    fn every_text_style_has_a_bundled_face() {
        for (name, style) in typography::ALL {
            assert!(
                find(style.family, style.weight, style.italic).is_some(),
                "text style {name} needs {} {} {} which isn't bundled",
                style.family,
                style.weight,
                if style.italic { "italic" } else { "roman" }
            );
        }
        // Due-soon times and low-priority marks switch the mono to 600 at the use site.
        assert!(find(MONO, 600, false).is_some());
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn every_face_is_embedded() {
        for f in FACES {
            assert!(f.bytes().len() > 10_000, "{} is not embedded", f.file);
        }
    }
}
