//! Case- and diacritic-insensitive text folding for free-text search.

const TABLE: &[(&str, char)] = &[
    ("àáâãäåāăą", 'a'),
    ("çćĉċč", 'c'),
    ("ďđ", 'd'),
    ("èéêëēĕėęě", 'e'),
    ("ĝğġģ", 'g'),
    ("ĥħ", 'h'),
    ("ìíîïĩīĭįı", 'i'),
    ("ĵ", 'j'),
    ("ķ", 'k'),
    ("ĺļľŀł", 'l'),
    ("ñńņňŉ", 'n'),
    ("òóôõöøōŏő", 'o'),
    ("ŕŗř", 'r'),
    ("śŝşš", 's'),
    ("ţťŧ", 't'),
    ("ùúûüũūŭůűų", 'u'),
    ("ŵ", 'w'),
    ("ýÿŷ", 'y'),
    ("źżž", 'z'),
];

/// Lowercases `s` and replaces common Latin letters with diacritics by their base letters, so that
/// `"mléko"` folds to `"mleko"`. Combining marks (decomposed input) are dropped.
#[must_use]
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            '\u{300}'..='\u{36f}' => {}
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            _ => match TABLE.iter().find(|(set, _)| set.contains(c)) {
                Some((_, base)) => out.push(*base),
                None => out.push(c),
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn folds_diacritics_and_case() {
        assert_eq!(fold("Mléko"), "mleko");
        assert_eq!(fold("ŽLUŤOUČKÝ kůň"), "zlutoucky kun");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("cafe\u{301}"), "cafe");
        assert_eq!(fold("plain"), "plain");
    }
}
