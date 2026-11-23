//! Desktop viewport and input mapping. Calendar and edit semantics live in todo-client.

use gpui::KeyDownEvent;
pub use todo_client::timeline::Axis;
use todo_client::timeline::Zoom;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Preferences {
    pub zoom: Zoom,
    pub tray: bool,
    pub rail: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: Zoom::default(),
            tray: false,
            rail: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Lane(isize),
    Shift(i64),
    Duration(i64),
    Today,
    Calendar(bool),
    Open,
    Complete,
    Escape,
}

pub fn key(event: &KeyDownEvent) -> Option<(Key, bool)> {
    let m = event.keystroke.modifiers;
    if m.control || m.platform || m.function {
        return None;
    }
    let step = if m.shift { 7 } else { 1 };
    let key = match (event.keystroke.key.as_str(), m.alt, m.shift) {
        ("up" | "k", false, false) => Key::Lane(-1),
        ("down" | "j", false, false) => Key::Lane(1),
        ("left", true, _) => Key::Shift(-step),
        ("right", true, _) => Key::Shift(step),
        ("[", false, false) => Key::Duration(-1),
        ("]", false, false) => Key::Duration(1),
        ("0", false, false) => Key::Today,
        ("d", false, starts) => Key::Calendar(starts),
        ("enter", false, false) => Key::Open,
        ("space", false, false) => Key::Complete,
        ("escape", false, false) => Key::Escape,
        _ => return None,
    };
    Some((key, !event.is_held || matches!(key, Key::Lane(_))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};
    use todo_client::{
        intent::workflow::{Dates, Schedule},
        timeline::{Span, SpanKind},
    };
    use todo_core::time::{Date, add_days};

    fn axis(zoom: Zoom) -> Axis {
        Axis {
            epoch: Date::constant(2026, 11, 19),
            offset: 0.,
            zoom,
            width: 840.,
        }
    }

    #[test]
    fn zoom_keeps_center_and_quarter_edits_still_snap_to_days() {
        let mut a = axis(Zoom::TwoWeeks);
        let center = a.date_at(420.);
        for zoom in Zoom::ALL {
            a = Axis {
                offset: a.zoom_offset(zoom),
                zoom,
                ..a
            };
            assert_eq!(a.date_at(420.), center);
        }
        assert_eq!(a.zoom, Zoom::Quarter);
        assert_eq!(
            a.zoom.column_start(center.expect("center date")).weekday(),
            jiff::civil::Weekday::Monday
        );
        assert_eq!(a.days_dragged(12.), 1);
        assert_eq!(a.days_dragged(-6.), -1);
        assert_eq!(a.date_at(20.), add_days(a.range().0, 2));
    }

    #[test]
    fn moving_and_resizing_map_pixels_to_inclusive_dates() {
        let a = axis(Zoom::TwoWeeks);
        let dates = Dates {
            start: None,
            due: a.date_at(120.),
        };
        let resized = Schedule::StartEdge(a.days_dragged(-90.))
            .preview(dates)
            .expect("milestone extended from its due date");
        let span = Span::from_dates(resized.start, resized.due).expect("resized span");
        assert_eq!(span.kind, SpanKind::Range);
        assert_eq!(span.in_window(a.range().0, a.range().1), Some((0, 3)));
        let moved = Schedule::Move(a.days_dragged(60.))
            .preview(resized)
            .expect("moved span");
        assert_eq!(moved.start, a.date_at(60.));
        assert_eq!(moved.due, a.date_at(180.));
        let offscreen = Span {
            first: add_days(a.epoch, 17).expect("start"),
            last: add_days(a.epoch, 19).expect("due"),
            kind: SpanKind::Range,
        };
        assert_eq!(a.reveal_offset(offscreen), 6.);
    }

    #[test]
    fn keyboard_shifts_by_day_or_week_without_repeating_writes() {
        let mut e = KeyDownEvent {
            keystroke: Keystroke {
                key: "left".into(),
                modifiers: Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert_eq!(key(&e), Some((Key::Shift(-1), true)));
        e.keystroke.modifiers.shift = true;
        assert_eq!(key(&e), Some((Key::Shift(-7), true)));
        e.is_held = true;
        assert_eq!(key(&e), Some((Key::Shift(-7), false)));
        e.keystroke.modifiers = Modifiers::default();
        e.keystroke.key = "down".into();
        assert_eq!(key(&e), Some((Key::Lane(1), true)));
        e.keystroke.key = "0".into();
        e.is_held = false;
        assert_eq!(key(&e), Some((Key::Today, true)));
        e.keystroke.key = "]".into();
        assert_eq!(key(&e), Some((Key::Duration(1), true)));
    }
}
