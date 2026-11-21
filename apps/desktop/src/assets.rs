//! Embedded assets: the single-colour stroke icons and hand-drawn list glyphs of the F2 design.
//!
//! GPUI renders each SVG as a mask and tints it with the element's text colour, so one file serves every
//! theme. The files live in `apps/desktop/assets/icons` until the shared assets directory lands.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

const FILES: &[(&str, &[u8])] = &[
    ("icons/sun.svg", include_bytes!("../assets/icons/sun.svg")),
    ("icons/star.svg", include_bytes!("../assets/icons/star.svg")),
    ("icons/cal.svg", include_bytes!("../assets/icons/cal.svg")),
    ("icons/list.svg", include_bytes!("../assets/icons/list.svg")),
    ("icons/done.svg", include_bytes!("../assets/icons/done.svg")),
    (
        "icons/spark.svg",
        include_bytes!("../assets/icons/spark.svg"),
    ),
    ("icons/chev.svg", include_bytes!("../assets/icons/chev.svg")),
    (
        "icons/chevr.svg",
        include_bytes!("../assets/icons/chevr.svg"),
    ),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    ("icons/flag.svg", include_bytes!("../assets/icons/flag.svg")),
    (
        "icons/repeat.svg",
        include_bytes!("../assets/icons/repeat.svg"),
    ),
    ("icons/bell.svg", include_bytes!("../assets/icons/bell.svg")),
    ("icons/hash.svg", include_bytes!("../assets/icons/hash.svg")),
    ("icons/mail.svg", include_bytes!("../assets/icons/mail.svg")),
    ("icons/undo.svg", include_bytes!("../assets/icons/undo.svg")),
    ("icons/more.svg", include_bytes!("../assets/icons/more.svg")),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    (
        "icons/check.svg",
        include_bytes!("../assets/icons/check.svg"),
    ),
    (
        "icons/diamond.svg",
        include_bytes!("../assets/icons/diamond.svg"),
    ),
    ("icons/min.svg", include_bytes!("../assets/icons/min.svg")),
    ("icons/max.svg", include_bytes!("../assets/icons/max.svg")),
    (
        "icons/sliders.svg",
        include_bytes!("../assets/icons/sliders.svg"),
    ),
    ("icons/ext.svg", include_bytes!("../assets/icons/ext.svg")),
    (
        "icons/sunrise.svg",
        include_bytes!("../assets/icons/sunrise.svg"),
    ),
    (
        "icons/enter.svg",
        include_bytes!("../assets/icons/enter.svg"),
    ),
    ("icons/link.svg", include_bytes!("../assets/icons/link.svg")),
    ("icons/moon.svg", include_bytes!("../assets/icons/moon.svg")),
    ("icons/note.svg", include_bytes!("../assets/icons/note.svg")),
    (
        "icons/clock.svg",
        include_bytes!("../assets/icons/clock.svg"),
    ),
    (
        "icons/glyph-work.svg",
        include_bytes!("../assets/icons/glyph-work.svg"),
    ),
    (
        "icons/glyph-projects.svg",
        include_bytes!("../assets/icons/glyph-projects.svg"),
    ),
    (
        "icons/glyph-personal.svg",
        include_bytes!("../assets/icons/glyph-personal.svg"),
    ),
    (
        "icons/glyph-planning.svg",
        include_bytes!("../assets/icons/glyph-planning.svg"),
    ),
    (
        "icons/glyph-finance.svg",
        include_bytes!("../assets/icons/glyph-finance.svg"),
    ),
    (
        "icons/glyph-office.svg",
        include_bytes!("../assets/icons/glyph-office.svg"),
    ),
    (
        "icons/glyph-events.svg",
        include_bytes!("../assets/icons/glyph-events.svg"),
    ),
    (
        "icons/glyph-todoapp.svg",
        include_bytes!("../assets/icons/glyph-todoapp.svg"),
    ),
    (
        "icons/glyph-home.svg",
        include_bytes!("../assets/icons/glyph-home.svg"),
    ),
    (
        "icons/glyph-reading.svg",
        include_bytes!("../assets/icons/glyph-reading.svg"),
    ),
    (
        "icons/glyph-errands.svg",
        include_bytes!("../assets/icons/glyph-errands.svg"),
    ),
    (
        "icons/glyph-health.svg",
        include_bytes!("../assets/icons/glyph-health.svg"),
    ),
    (
        "icons/glyph-travel.svg",
        include_bytes!("../assets/icons/glyph-travel.svg"),
    ),
    (
        "icons/glyph-admin.svg",
        include_bytes!("../assets/icons/glyph-admin.svg"),
    ),
];

/// The app's [`AssetSource`].
#[derive(Debug, Clone, Copy)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_an_svg_and_loads_by_path() {
        for (path, bytes) in FILES {
            let text = std::str::from_utf8(bytes).expect("utf-8");
            assert!(
                text.starts_with("<svg") && text.contains("viewBox=\"0 0 24 24\""),
                "{path}"
            );
            assert!(Assets.load(path).ok().flatten().is_some(), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").ok().flatten().is_none());
    }
}
