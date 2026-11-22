//! How the main window arranges its panes at its width.
//!
//! At `desktop::THREE_PANE_MIN` and wider, the sidebar, the room and the detail pane sit side by side. A
//! narrower window keeps the room at its full width: the detail pane opens over the right part of the room,
//! with a scrim over the rest of it, and the room header puts its counts under the title so the title keeps
//! its full size.

use crate::theme::{desktop, elevation, motion, space};

/// The window's width class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Width {
    /// Three panes side by side.
    #[default]
    Wide,
    /// Below the breakpoint: the detail pane opens over the room.
    Narrow,
}

impl Width {
    /// The class of a window `width` logical pixels wide.
    pub fn of(width: f32) -> Self {
        if width < desktop::THREE_PANE_MIN {
            Self::Narrow
        } else {
            Self::Wide
        }
    }

    /// The room header puts its done and open counts on a line of their own, under the title, so the
    /// title never gives way to them.
    pub fn counts_under_title(self) -> bool {
        self == Self::Narrow
    }
}

/// Where the detail pane goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailPane {
    /// Not drawn: the room takes its width.
    Hidden,
    /// A column beside the room: the selected task, or the "Select a task" placeholder.
    Docked,
    /// Over the right part of the room, with a scrim over the rest of it.
    Overlay,
}

impl DetailPane {
    /// Where the detail pane goes in a window of `width`, with a task `selected` or not, on the finished
    /// day or not. A wide window always shows the column, except on the finished day with nothing
    /// selected: an empty placeholder beside the dusk scene says nothing. A narrow window shows the pane
    /// only for a selected task.
    pub fn at(width: Width, selected: bool, finished_day: bool) -> Self {
        match width {
            Width::Wide if selected || !finished_day => Self::Docked,
            Width::Narrow if selected => Self::Overlay,
            _ => Self::Hidden,
        }
    }
}

/// The room pane's width (and so a row's) in a window `window` wide: the window less the sidebar and,
/// when it's docked, the detail pane, each with its hairline border. An overlay leaves the room as it is.
pub fn room_width(window: f32, detail: DetailPane) -> f32 {
    let mut width = window - desktop::SIDEBAR - elevation::HAIRLINE;
    if detail == DetailPane::Docked {
        width -= desktop::DETAIL + elevation::HAIRLINE;
    }
    width
}

/// How the detail overlay and its scrim move: opening and closing durations (ms) and how far the pane
/// slides in from the right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayMotion {
    pub open_ms: u32,
    pub close_ms: u32,
    pub slide: f32,
}

impl OverlayMotion {
    /// The overlay enters over `OVERLAY_IN` and leaves over `OVERLAY_OUT`, fading and sliding `S6` from the
    /// right edge. With reduced motion the slide goes and the fades shrink to `REDUCED_FADE`.
    pub fn new(reduced: bool) -> Self {
        if reduced {
            Self {
                open_ms: motion::reduced(motion::OVERLAY_IN, true),
                close_ms: motion::reduced(motion::OVERLAY_OUT, true),
                slide: 0.,
            }
        } else {
            Self {
                open_ms: motion::OVERLAY_IN,
                close_ms: motion::OVERLAY_OUT,
                slide: space::S6,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The narrowest window (`app::MIN_WINDOW`, which only builds on Windows).
    const MIN_WINDOW: f32 = 960.;

    #[test]
    fn the_breakpoint_is_the_three_pane_minimum() {
        assert_eq!(Width::of(MIN_WINDOW), Width::Narrow);
        assert_eq!(Width::of(desktop::THREE_PANE_MIN - 1.), Width::Narrow);
        assert_eq!(Width::of(desktop::THREE_PANE_MIN), Width::Wide);
        assert_eq!(
            Width::of(desktop::SIDEBAR + desktop::LIST + desktop::DETAIL),
            Width::Wide
        );
        // Three panes at the breakpoint still leave the list at least 560 wide.
        assert!(room_width(desktop::THREE_PANE_MIN, DetailPane::Docked) >= 556.);
    }

    #[test]
    fn a_narrow_window_puts_the_counts_under_the_title() {
        assert!(Width::Narrow.counts_under_title());
        assert!(!Width::Wide.counts_under_title());
    }

    #[test]
    fn a_wide_window_docks_the_detail_pane() {
        assert_eq!(DetailPane::at(Width::Wide, true, false), DetailPane::Docked);
        // The placeholder shows when nothing is selected...
        assert_eq!(
            DetailPane::at(Width::Wide, false, false),
            DetailPane::Docked
        );
        // ...except beside the finished day's dusk scene.
        assert_eq!(DetailPane::at(Width::Wide, false, true), DetailPane::Hidden);
        assert_eq!(DetailPane::at(Width::Wide, true, true), DetailPane::Docked);
    }

    #[test]
    fn a_narrow_window_opens_the_detail_over_the_room_only_for_a_task() {
        assert_eq!(
            DetailPane::at(Width::Narrow, true, false),
            DetailPane::Overlay
        );
        assert_eq!(
            DetailPane::at(Width::Narrow, true, true),
            DetailPane::Overlay
        );
        assert_eq!(
            DetailPane::at(Width::Narrow, false, false),
            DetailPane::Hidden
        );
        assert_eq!(
            DetailPane::at(Width::Narrow, false, true),
            DetailPane::Hidden
        );
    }

    #[test]
    fn the_room_keeps_its_width_under_an_overlay() {
        let narrow = MIN_WINDOW;
        let full = narrow - desktop::SIDEBAR - elevation::HAIRLINE;
        assert_eq!(room_width(narrow, DetailPane::Overlay), full);
        assert_eq!(room_width(narrow, DetailPane::Hidden), full);
        assert_eq!(
            room_width(narrow, DetailPane::Docked),
            full - desktop::DETAIL - elevation::HAIRLINE
        );
        // The reference window: the list column is the design's 800 (less the hairlines).
        let wide = desktop::SIDEBAR + desktop::LIST + desktop::DETAIL;
        assert_eq!(
            room_width(wide, DetailPane::Docked),
            desktop::LIST - 2. * elevation::HAIRLINE
        );
    }

    #[test]
    fn the_overlay_moves_with_the_overlay_tokens_and_respects_reduced_motion() {
        let full = OverlayMotion::new(false);
        assert_eq!(full.open_ms, motion::OVERLAY_IN);
        assert_eq!(full.close_ms, motion::OVERLAY_OUT);
        assert_eq!(full.slide, space::S6);
        let reduced = OverlayMotion::new(true);
        assert_eq!(reduced.slide, 0.);
        assert!(reduced.open_ms <= motion::REDUCED_FADE && reduced.open_ms > 0);
        assert!(reduced.close_ms <= motion::REDUCED_FADE && reduced.close_ms > 0);
    }
}
