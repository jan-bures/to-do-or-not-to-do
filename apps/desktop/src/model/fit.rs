//! Fitting a row's metadata line into the width it has, without clipping anything mid-glyph.
//!
//! The view measures each item with the text system and asks how many tags fit; the tags that don't are
//! counted as `+N` instead of being cut. Widths are in logical pixels.

/// How many tags a metadata line shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagFit {
    /// The first `shown` tags are drawn whole.
    pub shown: usize,
    /// The rest, drawn as a `+N` count; zero when every tag fits, or when not even the count fits.
    pub more: usize,
}

/// Fits `tags` (their widths, in order) into `room`, each one after a `gap`. When they don't all fit, the
/// line keeps as many as leave room for a `+N` count of the rest (`count(n)` is the count's width), so a
/// tag is either whole or counted, never cut. If not even the count fits, nothing is shown.
pub fn fit_tags(room: f32, tags: &[f32], gap: f32, count: impl Fn(usize) -> f32) -> TagFit {
    let all: f32 = tags.iter().map(|w| gap + w).sum();
    if all <= room {
        return TagFit {
            shown: tags.len(),
            more: 0,
        };
    }
    let mut used = 0.;
    let mut best = None;
    for shown in 0..tags.len() {
        if used + gap + count(tags.len() - shown) <= room {
            best = Some(shown);
        }
        used += gap + tags[shown];
        if used > room {
            break;
        }
    }
    match best {
        Some(shown) => TagFit {
            shown,
            more: tags.len() - shown,
        },
        None => TagFit { shown: 0, more: 0 },
    }
}

/// The `+N` label for tags that didn't fit.
pub fn more_label(n: usize) -> String {
    format!("+{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A count is 20 px wide, whatever the number.
    fn count(_: usize) -> f32 {
        20.
    }

    #[test]
    fn every_tag_shows_when_they_all_fit() {
        assert_eq!(
            fit_tags(100., &[30., 30.], 8., count),
            TagFit { shown: 2, more: 0 }
        );
        // Exactly full still fits.
        assert_eq!(
            fit_tags(76., &[30., 30.], 8., count),
            TagFit { shown: 2, more: 0 }
        );
        assert_eq!(fit_tags(0., &[], 8., count), TagFit { shown: 0, more: 0 });
    }

    #[test]
    fn tags_that_dont_fit_become_a_count() {
        // 8 + 30 + 8 + 30 + 8 + 30 = 114 > 100. Two tags and a count need 38 + 38 + 28 = 104; one tag and a
        // count need 38 + 28 = 66.
        assert_eq!(
            fit_tags(100., &[30., 30., 30.], 8., count),
            TagFit { shown: 1, more: 2 }
        );
        assert_eq!(
            fit_tags(104., &[30., 30., 30.], 8., count),
            TagFit { shown: 2, more: 1 }
        );
    }

    #[test]
    fn a_long_first_tag_is_counted_rather_than_cut() {
        assert_eq!(
            fit_tags(60., &[120., 10.], 8., count),
            TagFit { shown: 0, more: 2 }
        );
    }

    #[test]
    fn nothing_shows_when_not_even_the_count_fits() {
        assert_eq!(
            fit_tags(20., &[30., 30.], 8., count),
            TagFit { shown: 0, more: 0 }
        );
        assert_eq!(
            fit_tags(-5., &[30.], 8., count),
            TagFit { shown: 0, more: 0 }
        );
    }

    #[test]
    fn a_wider_count_for_more_tags_is_respected() {
        // "+12" is wider than "+2": the count's width depends on the number it shows.
        let tags = [10.; 12];
        let count = |n: usize| if n >= 10 { 40. } else { 20. };
        // Two tags (36) and a wide "+10" (8 + 40) need 84; one tag (18) and "+11" need 66.
        assert_eq!(
            fit_tags(80., &tags, 8., count),
            TagFit { shown: 1, more: 11 }
        );
        // At 84, three tags (54) and the narrower "+9" (8 + 20) need only 82.
        assert_eq!(
            fit_tags(84., &tags, 8., count),
            TagFit { shown: 3, more: 9 }
        );
        // Never more than the room, and every tag is accounted for.
        for room in 0..200 {
            let room = room as f32;
            let fit = fit_tags(room, &tags, 8., count);
            let used: f32 = tags[..fit.shown].iter().map(|w| 8. + w).sum::<f32>()
                + if fit.more > 0 {
                    8. + count(fit.more)
                } else {
                    0.
                };
            assert!(used <= room || (fit.shown == 0 && fit.more == 0), "{room}");
            assert!(fit.shown + fit.more == tags.len() || (fit.shown == 0 && fit.more == 0));
        }
    }

    #[test]
    fn more_label_counts() {
        assert_eq!(more_label(3), "+3");
    }
}
