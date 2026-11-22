//! What a list or collection looks like: its hue (one of the design's swatches) and its hand-drawn glyph,
//! and how an agent is shown.
//!
//! The data model stores a free-form `color`. The app writes a swatch key there (`"lagoon"`); imported data
//! may carry a hex colour (`"#3366ff"`), which snaps to the nearest swatch by hue. Without a colour the
//! swatch is picked from the ID, so it is stable across devices. Glyphs aren't stored: they are picked
//! from the name (English and Czech words), with a quiet default.

use todo_core::Actor;
use todo_core::query::fold;

use crate::theme::{Agent, Glyph, Hue, hues};

/// A collection's or list's hue: its swatch key, a hex colour snapped to the nearest swatch, or a swatch
/// picked from `seed` (the entity's ID).
pub fn hue(color: Option<&str>, seed: &[u8]) -> Hue {
    let named = color.and_then(|c| {
        let c = c.trim();
        hues::SWATCHES
            .iter()
            .find(|s| s.key.eq_ignore_ascii_case(c))
            .map(|s| s.degrees)
            .or_else(|| hex_hue(c).map(nearest_swatch))
    });
    named.unwrap_or_else(|| hues::swatch_for_seed(seed).degrees)
}

fn nearest_swatch(hue: Hue) -> Hue {
    hues::SWATCHES
        .iter()
        .min_by(|a, b| hue_distance(a.degrees, hue).total_cmp(&hue_distance(b.degrees, hue)))
        .map_or(hue, |s| s.degrees)
}

fn hue_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The OKLCH hue of `#rrggbb`, or `None` for anything else (or a grey, which has no hue).
fn hex_hue(s: &str) -> Option<Hue> {
    let rgb = todo_design::Rgb::from_hex(s)?;
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb.r), lin(rgb.g), lin(rgb.b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let ok_a = 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s;
    let ok_b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s;
    if (ok_a * ok_a + ok_b * ok_b).sqrt() < 0.02 {
        return None;
    }
    Some(ok_b.atan2(ok_a).to_degrees().rem_euclid(360.0))
}

/// Words (folded: lowercase, no diacritics) that pick a glyph, checked in order.
const GLYPH_WORDS: &[(Glyph, &[&str])] = &[
    (Glyph::Sun, &["my day", "today", "dnes"]),
    (Glyph::Briefcase, &["work", "job", "prace", "zamestn"]),
    (
        Glyph::Coin,
        &[
            "financ", "money", "budget", "tax", "invoice", "penize", "dane", "ucty", "faktur",
        ],
    ),
    (Glyph::Paperclip, &["office", "kancel"]),
    (
        Glyph::CalendarStar,
        &["event", "party", "akce", "udalost", "oslav"],
    ),
    (
        Glyph::Code,
        &["code", "dev", "app", "software", "program", "github"],
    ),
    (
        Glyph::Wrench,
        &[
            "renovat", "repair", "diy", "fix", "oprav", "rekonstr", "dilna",
        ],
    ),
    (
        Glyph::Book,
        &["read", "book", "kniha", "knihy", "cteni", "studi", "learn"],
    ),
    (
        Glyph::Basket,
        &[
            "shop", "grocer", "errand", "buy", "nakup", "potravin", "obchod",
        ],
    ),
    (
        Glyph::Heart,
        &[
            "health", "doctor", "fitness", "sport", "zdravi", "lekar", "cvic",
        ],
    ),
    (
        Glyph::PaperPlane,
        &[
            "travel", "trip", "vacation", "holiday", "cest", "dovolen", "vylet",
        ],
    ),
    (
        Glyph::Document,
        &["admin", "document", "paper", "doklad", "urad", "formular"],
    ),
    (Glyph::Flag, &["plan", "goal", "cil", "milestone"]),
    (Glyph::Layers, &["project", "projekt"]),
    (
        Glyph::House,
        &[
            "personal", "home", "house", "family", "osobni", "domov", "doma", "rodina",
        ],
    ),
];

fn glyph_for(name: &str) -> Option<Glyph> {
    let folded = fold(name);
    GLYPH_WORDS
        .iter()
        .find(|(_, words)| words.iter().any(|w| folded.contains(w)))
        .map(|(g, _)| *g)
}

/// A list's glyph: from its name, or a document.
pub fn list_glyph(name: &str) -> Glyph {
    glyph_for(name).unwrap_or(Glyph::Document)
}

/// A collection's glyph: from its name, or layers.
pub fn collection_glyph(name: &str) -> Glyph {
    glyph_for(name).unwrap_or(Glyph::Layers)
}

/// The letter mark for an actor, if it is an agent.
pub fn agent(actor: &Actor) -> Option<Agent> {
    let id = actor.agent_id()?.as_str();
    Some(if id.starts_with("claude") {
        Agent::Claude
    } else if id.starts_with("codex") {
        Agent::Codex
    } else if id == "pi" || id.starts_with("pi-") {
        Agent::Pi
    } else {
        let letter = id.chars().find(char::is_ascii_alphanumeric).unwrap_or('?');
        let i = fnv(id.as_bytes()) as usize % hues::SWATCHES.len();
        // Whole-degree hues all pass the mark's contrast checks.
        Agent::Other {
            letter,
            hue: hues::SWATCHES[i].degrees as u16,
        }
    })
}

/// An agent's display name: "Claude", "Codex", "Pi" or its ID with a capital.
pub fn agent_name(actor: &Actor) -> Option<String> {
    let id = actor.agent_id()?.as_str();
    Some(match agent(actor)? {
        Agent::Claude => "Claude".into(),
        Agent::Codex => "Codex".into(),
        Agent::Pi => "Pi".into(),
        Agent::Other { .. } => {
            let mut c = id.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swatch(key: &str) -> Hue {
        hues::SWATCHES
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.degrees)
            .expect("swatch")
    }

    #[test]
    fn hues_come_from_swatches() {
        assert_eq!(hue(Some("lagoon"), b"x"), swatch("lagoon"));
        assert_eq!(hue(Some("Rose"), b"x"), swatch("rose"));
        // A hex colour snaps to the nearest swatch: a pure blue is cornflower, a red rose or coral.
        assert_eq!(hue(Some("#3366ff"), b"x"), swatch("cornflower"));
        assert!([swatch("rose"), swatch("coral")].contains(&hue(Some("#e53935"), b"x")));
        // Grey has no hue; the ID picks one, the same one every time.
        let a = hue(Some("#808080"), b"list-1");
        assert_eq!(a, hue(None, b"list-1"));
        assert!(hues::SWATCHES.iter().any(|s| s.degrees == a));
    }

    #[test]
    fn glyphs_come_from_names_in_both_languages() {
        assert_eq!(collection_glyph("Work"), Glyph::Briefcase);
        assert_eq!(collection_glyph("Práce"), Glyph::Briefcase);
        assert_eq!(collection_glyph("Projects"), Glyph::Layers);
        assert_eq!(collection_glyph("Osobní"), Glyph::House);
        assert_eq!(list_glyph("Nákupy"), Glyph::Basket);
        assert_eq!(list_glyph("Zdraví"), Glyph::Heart);
        assert_eq!(list_glyph("Home renovation"), Glyph::Wrench);
        assert_eq!(list_glyph("Reading list"), Glyph::Book);
        assert_eq!(list_glyph("Misc"), Glyph::Document);
        assert_eq!(collection_glyph("Misc"), Glyph::Layers);
    }

    #[test]
    fn agents_get_marks_and_names() {
        let claude = Actor::agent("claude-code").expect("id");
        assert_eq!(agent(&claude), Some(Agent::Claude));
        assert_eq!(agent_name(&claude).as_deref(), Some("Claude"));
        assert_eq!(agent(&Actor::agent("pi").expect("id")), Some(Agent::Pi));
        let other = Actor::agent("zephyr").expect("id");
        assert!(matches!(
            agent(&other),
            Some(Agent::Other { letter: 'z', .. })
        ));
        assert_eq!(agent_name(&other).as_deref(), Some("Zephyr"));
        assert_eq!(agent(&Actor::owner("desk").expect("id")), None);
    }
}
