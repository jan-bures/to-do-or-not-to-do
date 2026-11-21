//! The bundled fonts, embedded from the shared `assets/fonts` through `todo_design::fonts` (the
//! `embed-fonts` feature). The app never uses system fonts.

use std::borrow::Cow;

use gpui::App;
use todo_design::fonts::FACES;

use crate::theme::ty;

/// The families the app asks for.
pub const FAMILIES: [&str; 4] = [ty::DISPLAY, ty::DISPLAY_TEXT, ty::UI, ty::MONO];

/// Registers every bundled face with GPUI's text system. Returns the families that are still missing
/// afterwards (empty when all is well).
pub fn load(cx: &App) -> Vec<&'static str> {
    let faces: Vec<Cow<'static, [u8]>> = FACES.iter().map(|f| Cow::Borrowed(f.bytes())).collect();
    if let Err(e) = cx.text_system().add_fonts(faces) {
        tracing::warn!("loading the bundled fonts failed: {e:#}");
    }
    let names = cx.text_system().all_font_names();
    FAMILIES
        .into_iter()
        .filter(|family| !names.iter().any(|n| n == family))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_is_embedded_and_every_family_is_bundled() {
        for face in FACES {
            assert!(face.bytes().len() > 10_000, "{}", face.file);
        }
        for family in FAMILIES {
            assert!(FACES.iter().any(|f| f.family == family), "{family}");
        }
    }
}
