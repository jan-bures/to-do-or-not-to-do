//! The bundled fonts and icons: every face in `fonts::FACES` exists, carries the family, weight and slant the
//! tokens ask for, and covers Czech; every icon file is listed in the enums and is a single-colour stroke SVG.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use todo_design::fonts::FACES;
use todo_design::{Glyph, Icon};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Czech letters with diacritics, both cases, plus the agent mark π for the mono face.
const CZECH: &str = "áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽ";

#[test]
fn fonts_cover_czech_and_match_the_tokens() {
    let mut listed = BTreeSet::new();
    for face in FACES {
        listed.insert(face.file.to_owned());
        let data = std::fs::read(repo().join(face.path()))
            .unwrap_or_else(|e| panic!("{}: {e}", face.path()));
        let f = ttf_parser::Face::parse(&data, 0).unwrap_or_else(|e| panic!("{}: {e}", face.file));

        let missing: String = CZECH
            .chars()
            .filter(|&c| f.glyph_index(c).is_none())
            .collect();
        assert!(missing.is_empty(), "{} lacks {missing}", face.file);
        if face.family == todo_design::typography::MONO {
            assert!(f.glyph_index('π').is_some(), "{} lacks π", face.file);
        }

        let family = f
            .names()
            .into_iter()
            .filter(|n| n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
            .find_map(|n| n.to_string());
        assert_eq!(family.as_deref(), Some(face.family), "{}", face.file);
        assert_eq!(f.weight().to_number(), face.weight, "{}", face.file);
        assert_eq!(f.is_italic(), face.italic, "{}", face.file);
        assert!(
            !f.is_variable(),
            "{} should be a static instance",
            face.file
        );
    }

    let on_disk: BTreeSet<String> = std::fs::read_dir(repo().join("assets/fonts"))
        .expect("assets/fonts")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".ttf"))
        .collect();
    assert_eq!(on_disk, listed, "font files on disk vs fonts::FACES");
}

fn svg_names(dir: &str) -> BTreeSet<String> {
    std::fs::read_dir(repo().join(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".svg").map(str::to_owned))
        .collect()
}

fn assert_single_colour(name: &str, svg: &str) {
    assert!(
        svg.contains("viewBox=\"0 0 24 24\""),
        "{name}: 24-unit grid"
    );
    assert!(svg.contains("stroke=\"currentColor\""), "{name}: tintable");
    for attr in svg.split_whitespace().filter(|a| a.starts_with("fill=")) {
        assert_eq!(
            attr.trim_end_matches('>'),
            "fill=\"none\"",
            "{name}: no fills"
        );
    }
    assert!(!svg.contains("#"), "{name}: no hard-coded colours");
}

#[test]
fn icons_are_listed_and_single_colour() {
    let listed: BTreeSet<String> = Icon::ALL.iter().map(|i| i.key().to_owned()).collect();
    assert_eq!(svg_names("assets/icons/lucide"), listed);
    for icon in Icon::ALL {
        assert_eq!(Icon::from_key(icon.key()), Some(*icon));
        assert_single_colour(icon.key(), icon.svg());
        assert!(
            icon.svg().contains("stroke-width=\"1.6\""),
            "{}",
            icon.key()
        );
    }

    let listed: BTreeSet<String> = Glyph::ALL.iter().map(|g| g.key().to_owned()).collect();
    assert_eq!(svg_names("assets/icons/glyphs"), listed);
    for glyph in Glyph::ALL {
        assert_single_colour(glyph.key(), glyph.svg());
    }
}
