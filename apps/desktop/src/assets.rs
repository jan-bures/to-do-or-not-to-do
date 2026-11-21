//! The app's [`AssetSource`]: the shared Lucide icons and hand-drawn list glyphs from `todo_design`.
//!
//! GPUI renders each SVG as a mask and tints it with the element's text colour, so one file serves every
//! theme. Paths are `icons/<key>.svg` for [`Icon`] and `glyphs/<key>.svg` for [`Glyph`].

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use todo_design::{Glyph, Icon};

const ICONS: &str = "icons/";
const GLYPHS: &str = "glyphs/";

/// The asset path of an icon.
pub fn icon_path(icon: Icon) -> SharedString {
    format!("{ICONS}{}.svg", icon.key()).into()
}

/// The asset path of a list glyph.
pub fn glyph_path(glyph: Glyph) -> SharedString {
    format!("{GLYPHS}{}.svg", glyph.key()).into()
}

fn svg(path: &str) -> Option<&'static str> {
    let key = |prefix: &str| path.strip_prefix(prefix)?.strip_suffix(".svg");
    if let Some(k) = key(ICONS) {
        Icon::from_key(k).map(Icon::svg)
    } else {
        key(GLYPHS).and_then(Glyph::from_key).map(Glyph::svg)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(svg(path).map(|s| Cow::Borrowed(s.as_bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let icons = Icon::ALL.iter().map(|i| icon_path(*i));
        let glyphs = Glyph::ALL.iter().map(|g| glyph_path(*g));
        Ok(icons
            .chain(glyphs)
            .filter(|p| p.starts_with(path))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_and_glyph_loads_by_path() {
        let all = Assets.list("").expect("list");
        assert_eq!(all.len(), Icon::ALL.len() + Glyph::ALL.len());
        for path in &all {
            let bytes = Assets.load(path).ok().flatten().expect("loads");
            assert!(
                std::str::from_utf8(&bytes).is_ok_and(|s| s.contains("<svg")),
                "{path}"
            );
        }
        assert_eq!(
            Assets
                .load(&icon_path(Icon::Sun))
                .ok()
                .flatten()
                .map(|b| b.len()),
            Some(Icon::Sun.svg().len())
        );
        assert!(Assets.load("icons/nope.svg").ok().flatten().is_none());
        assert!(Assets.load("glyphs/sun.png").ok().flatten().is_none());
        assert_eq!(
            Assets.list("glyphs/").expect("list").len(),
            Glyph::ALL.len()
        );
    }
}
