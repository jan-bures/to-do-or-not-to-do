//! What a list or collection looks like (its swatch and hand-drawn glyph) and how an agent is shown. The
//! same rules as the desktop app:
//!
//! - The data model stores a free-form `color`. A swatch key (`"lagoon"`) is used as is; a hex colour
//!   (`"#3366ff"`, from the importer) snaps to the nearest swatch by OKLCH hue; without a colour the swatch
//!   is picked from the ID, so every device shows the same one.
//! - Glyphs aren't stored: they're picked from the name (English and Czech words), with a quiet default.
//!
//! Every hue the app shows is a named one, so Kotlin looks its colours up by key in the generated tokens.

use todo_core::Actor;
use todo_core::query::fold;
use todo_design::{Glyph, Rgb, hues};

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
fn hex_hue(s: &str) -> Option<f64> {
    let rgb = Rgb::from_hex(s)?;
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

/// The swatch key of a collection or list: its stored swatch key, a hex colour snapped to the nearest
/// swatch, or a swatch picked from `seed` (the entity's ID).
pub fn swatch(color: Option<&str>, seed: &[u8]) -> &'static str {
    let named = color.and_then(|c| {
        let c = c.trim();
        hues::SWATCHES
            .iter()
            .find(|s| s.key.eq_ignore_ascii_case(c))
            .or_else(|| {
                let h = hex_hue(c)?;
                hues::SWATCHES.iter().min_by(|a, b| {
                    hue_distance(a.degrees, h).total_cmp(&hue_distance(b.degrees, h))
                })
            })
            .map(|s| s.key)
    });
    named.unwrap_or_else(|| hues::swatch_for_seed(seed).key)
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

/// A list's glyph key: from its name, or a document.
pub fn list_glyph(name: &str) -> &'static str {
    glyph_for(name).unwrap_or(Glyph::Document).key()
}

/// A collection's glyph key: from its name, or layers.
pub fn collection_glyph(name: &str) -> &'static str {
    glyph_for(name).unwrap_or(Glyph::Layers).key()
}

/// An agent's round letter mark.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AgentMark {
    /// The letter: `C`, `X`, `π` or the agent ID's first letter.
    pub letter: String,
    /// "Claude", "Codex", "Pi" or the ID with a capital.
    pub name: String,
    /// Token key of its colours: `claude`, `codex` or `pi` (agent colours), or a swatch key for any
    /// other agent (then the list colours of that swatch are used).
    pub hue: String,
    /// True when `hue` names an agent palette, false for a swatch.
    pub known: bool,
}

/// The letter mark for an actor, if it is an agent.
pub fn agent(actor: &Actor) -> Option<AgentMark> {
    let id = actor.agent_id()?.as_str();
    let known = |letter: &str, name: &str, hue: &str| AgentMark {
        letter: letter.into(),
        name: name.into(),
        hue: hue.into(),
        known: true,
    };
    Some(if id.starts_with("claude") {
        known("C", "Claude", "claude")
    } else if id.starts_with("codex") {
        known("X", "Codex", "codex")
    } else if id == "pi" || id.starts_with("pi-") {
        known("π", "Pi", "pi")
    } else {
        let letter = id
            .chars()
            .find(char::is_ascii_alphanumeric)
            .unwrap_or('?')
            .to_ascii_uppercase();
        let mut c = id.chars();
        let name = c
            .next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default();
        AgentMark {
            letter: letter.to_string(),
            name,
            hue: swatch(None, id.as_bytes()).into(),
            known: false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swatches_from_keys_hex_and_ids() {
        assert_eq!(swatch(Some("lagoon"), b"x"), "lagoon");
        assert_eq!(swatch(Some("Rose"), b"x"), "rose");
        assert_eq!(swatch(Some("#3366ff"), b"x"), "cornflower");
        assert!(["rose", "coral"].contains(&swatch(Some("#e53935"), b"x")));
        // Grey has no hue: the ID picks one, the same every time.
        assert_eq!(swatch(Some("#808080"), b"list-1"), swatch(None, b"list-1"));
    }

    #[test]
    fn glyphs_from_names_in_both_languages() {
        assert_eq!(collection_glyph("Work"), "briefcase");
        assert_eq!(collection_glyph("Práce"), "briefcase");
        assert_eq!(collection_glyph("Osobní"), "house");
        assert_eq!(list_glyph("Nákupy"), "basket");
        assert_eq!(list_glyph("Zdraví"), "heart");
        assert_eq!(list_glyph("Misc"), "document");
        assert_eq!(collection_glyph("Misc"), "layers");
    }

    #[test]
    fn agents_get_marks() {
        let m = agent(&Actor::agent("claude-code").unwrap()).unwrap();
        assert_eq!((m.letter.as_str(), m.hue.as_str()), ("C", "claude"));
        assert_eq!(agent(&Actor::agent("pi").unwrap()).unwrap().letter, "π");
        let z = agent(&Actor::agent("zephyr").unwrap()).unwrap();
        assert_eq!(
            (z.letter.as_str(), z.name.as_str(), z.known),
            ("Z", "Zephyr", false)
        );
        assert!(hues::SWATCHES.iter().any(|s| s.key == z.hue));
        assert_eq!(agent(&Actor::owner("phone").unwrap()), None);
    }
}
