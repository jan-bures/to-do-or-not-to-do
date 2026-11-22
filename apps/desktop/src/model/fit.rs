//! Fitting a row's metadata line into the width it has, without clipping anything mid-glyph.
//!
//! The view measures each item with the text system and asks what fits. Tags that don't fit whole are
//! counted as `+N` instead of being cut; when even the items before them don't fit, they give way in a fixed
//! order (the mail mark, then the step meter, then the step count) and the list chip narrows last. Widths
//! are in logical pixels. GPUI rounds each element's size up to a whole pixel, so every width is rounded up
//! here too before it is added: summing raw widths undercounts a line of many small items.

use crate::theme::desktop;

/// In a squeezed room, use the empty space below the trailing columns for the metadata line.
/// Keep its left edge aligned with the title in either layout.
pub fn row_meta_width(room: f32, title_width: f32) -> f32 {
    if room < desktop::LIST_MIN {
        room - 2. * desktop::LIST_PAD_X - desktop::COL_CHECK - desktop::ROW_GAP
    } else {
        title_width
    }
}

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
    let all: f32 = tags.iter().map(|w| gap + w.ceil()).sum();
    if all <= room {
        return TagFit {
            shown: tags.len(),
            more: 0,
        };
    }
    let mut used = 0.;
    let mut best = None;
    for shown in 0..tags.len() {
        if used + gap + count(tags.len() - shown).ceil() <= room {
            best = Some(shown);
        }
        used += gap + tags[shown].ceil();
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

/// A metadata line's items, measured, in drawing order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaItems {
    /// The gap between items.
    pub gap: f32,
    /// The list chip: its natural width (already capped) and the narrowest it may get (its dot and an
    /// ellipsis).
    pub chip: Option<(f32, f32)>,
    /// Steps: the meter's width, the gap between meter and count, and the count's width.
    pub steps: Option<(f32, f32, f32)>,
    /// The mail mark.
    pub mail: Option<f32>,
    /// The tags.
    pub tags: Vec<f32>,
}

/// What a metadata line draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetaFit {
    /// The chip's width, if it shows.
    pub chip: Option<f32>,
    pub meter: bool,
    pub count: bool,
    pub mail: bool,
    pub tags: TagFit,
}

impl MetaFit {
    /// The width the line takes, the way [`fit_meta`] counts it.
    pub fn width(&self, items: &MetaItems, count: impl Fn(usize) -> f32) -> f32 {
        let mut parts: Vec<f32> = Vec::new();
        parts.extend(self.chip.map(f32::ceil));
        if let Some((meter, inner, n)) = items.steps {
            match (self.meter, self.count) {
                (true, true) => parts.push(meter.ceil() + inner + n.ceil()),
                (true, false) => parts.push(meter.ceil()),
                (false, true) => parts.push(n.ceil()),
                (false, false) => {}
            }
        }
        if self.mail {
            parts.extend(items.mail.map(f32::ceil));
        }
        parts.extend(items.tags[..self.tags.shown].iter().map(|w| w.ceil()));
        if self.tags.more > 0 {
            parts.push(count(self.tags.more).ceil());
        }
        let gaps = parts.len().saturating_sub(1) as f32 * items.gap;
        parts.iter().sum::<f32>() + gaps
    }
}

/// Fits a metadata line into `room` (the title column's width). Tags take whatever the other items leave,
/// so they go first; if the other items alone don't fit, the mail mark goes, then the step meter (the count
/// stays), then the count, and then the chip narrows, down to its minimum. A line narrower than even
/// that drops the chip as well. The result is never wider than `room`.
pub fn fit_meta(room: f32, items: &MetaItems, count: impl Fn(usize) -> f32) -> MetaFit {
    let gap = items.gap;
    let none = TagFit { shown: 0, more: 0 };
    let mut fit = MetaFit {
        chip: items.chip.map(|(natural, _)| natural.ceil()),
        meter: items.steps.is_some(),
        count: items.steps.is_some(),
        mail: items.mail.is_some(),
        tags: none,
    };
    let fits = |fit: &MetaFit| fit.width(items, &count) <= room;
    let give_way: [fn(&mut MetaFit); 3] =
        [|f| f.mail = false, |f| f.meter = false, |f| f.count = false];
    for drop in give_way {
        if fits(&fit) {
            break;
        }
        drop(&mut fit);
    }
    if !fits(&fit) {
        // Only the chip is left, and it's too wide: it narrows to the room, but no further than its minimum.
        fit.chip = items
            .chip
            .map(|(_, min)| min.ceil())
            .filter(|&min| min <= room)
            .map(|min| room.floor().max(min));
    }
    // Whatever is left goes to the tags, each after a gap (the first needs none when nothing precedes it).
    let used = fit.width(items, &count);
    let leading = fit.chip.is_some() || fit.meter || fit.count || fit.mail;
    let left = room - used + if leading { 0. } else { gap };
    fit.tags = fit_tags(left, &items.tags, gap, &count);
    fit
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
    fn widths_are_rounded_up_like_gpui_lays_them_out() {
        // 36 one-character tags, each 7.4 px: raw, they and their gaps add up to 554.4, but GPUI gives each
        // a whole 8 px, so the line really takes 576 and doesn't fit in 560.
        let tags = [7.4; 36];
        let fit = fit_tags(560., &tags, 8., count);
        assert!(fit.shown < 36, "{fit:?}");
        let used: f32 = tags[..fit.shown].iter().map(|w| 8. + w.ceil()).sum::<f32>()
            + if fit.more > 0 {
                8. + count(fit.more)
            } else {
                0.
            };
        assert!(used <= 560., "{used}");
        assert_eq!(fit.shown + fit.more, 36);
        // With room for the rounded widths, they all show.
        assert_eq!(fit_tags(576., &tags, 8., count).shown, 36);
    }

    /// A Q4-roadmap row: a capped chip, a 2/5 meter, the mail mark and three tags.
    fn row() -> MetaItems {
        MetaItems {
            gap: 8.,
            chip: Some((180., 36.)),
            steps: Some((23., 6., 21.5)),
            mail: Some(14.),
            tags: vec![65.2, 22.3, 58.9],
        }
    }

    #[test]
    fn a_wide_line_shows_everything() {
        let fit = fit_meta(600., &row(), count);
        assert_eq!(fit.chip, Some(180.));
        assert!(fit.meter && fit.count && fit.mail);
        assert_eq!(fit.tags, TagFit { shown: 3, more: 0 });
    }

    #[test]
    fn crowded_docked_rows_can_show_a_chip_steps_mail_and_a_tag_count() {
        let room = desktop::LIST_MIN_BESIDE_DETAIL;
        let title_width = room
            - (2. * desktop::LIST_PAD_X
                + desktop::COL_CHECK
                + desktop::COL_AGENT
                + desktop::COL_PRIORITY
                + desktop::COL_DUE
                + 4. * desktop::ROW_GAP);
        let width = row_meta_width(room, title_width);
        let items = row();
        let fit = fit_meta(width, &items, count);
        assert_eq!(fit.chip, Some(180.));
        assert!(fit.meter && fit.count && fit.mail);
        assert_eq!(fit.tags, TagFit { shown: 0, more: 3 });
        assert!(fit.width(&items, count) <= width);
        assert_eq!(row_meta_width(desktop::LIST_MIN, title_width), title_width);
    }

    #[test]
    fn items_give_way_in_order_and_the_chip_narrows_last() {
        let items = row();
        // chip 180, steps 23 + 6 + 22 = 51, mail 14, with gaps: 180 + 8 + 51 + 8 + 14 = 261.
        let at = |room: f32| fit_meta(room, &items, count);
        // Tags go first: at 261 everything but the tags fits, and there's no room even for a count.
        let f = at(261.);
        assert!(f.mail && f.meter && f.count);
        assert_eq!(f.tags, TagFit { shown: 0, more: 0 });
        // Then the mail mark.
        let f = at(260.);
        assert!(!f.mail && f.meter && f.count);
        // Then the meter, keeping the count: 180 + 8 + 51 = 239 doesn't fit in 238; 180 + 8 + 22 = 210 does.
        let f = at(238.);
        assert!(!f.mail && !f.meter && f.count);
        // Then the count.
        let f = at(209.);
        assert!(!f.meter && !f.count);
        assert_eq!(f.chip, Some(180.));
        // Then the chip narrows to the room, down to its minimum.
        assert_eq!(at(120.).chip, Some(120.));
        assert_eq!(at(38.).chip, Some(38.));
        assert_eq!(at(36.).chip, Some(36.));
        // Narrower than that, the chip goes too.
        assert_eq!(at(35.).chip, None);
    }

    #[test]
    fn the_line_is_never_wider_than_its_room() {
        let items = row();
        let mut tags = items.clone();
        tags.tags = vec![7.4; 36];
        for items in [items, tags] {
            for room in 0..700 {
                let room = room as f32 / 1.5;
                let fit = fit_meta(room, &items, count);
                let width = fit.width(&items, count);
                assert!(width <= room, "{room}: {fit:?} takes {width}");
            }
        }
    }

    #[test]
    fn more_label_counts() {
        assert_eq!(more_label(3), "+3");
    }
}
