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

/// How the narrow window's detail overlay is drawn in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayPhase {
    /// A task was just opened: the pane slides and fades in.
    Entering,
    /// Drawn at rest: a task was already open when the window narrowed past the breakpoint (or the
    /// screenshot fixtures, which draw everything at rest).
    Resting,
    /// The task was closed: its pane fades out, inert, while nothing in it takes input.
    Leaving,
}

impl OverlayPhase {
    /// Whether the pane takes clicks and keys. A pane fading out never does: a click on its fading
    /// checkbox or step must not change a task that was already dismissed.
    pub fn interactive(self) -> bool {
        self != Self::Leaving
    }

    /// How an overlay that wasn't drawn in the last frame starts: it slides in when a task opened, but
    /// is drawn at rest when the window was wide with a task already open (a resize crossed the
    /// breakpoint), so the pane doesn't replay its entrance.
    pub fn on_mount(last_width: Width, last_selected: bool) -> Self {
        if last_width == Width::Wide && last_selected {
            Self::Resting
        } else {
            Self::Entering
        }
    }
}

/// What the narrow window draws over the room, and how: the open task's detail (`open`, in `entry`
/// phase) while the detail pane is an overlay, else the detail just closed (`closing`), fading out and
/// inert, as long as the window is narrow. Nothing in a wide window.
pub fn overlay<T>(
    pane: DetailPane,
    width: Width,
    open: Option<T>,
    entry: OverlayPhase,
    closing: Option<T>,
) -> Option<(OverlayPhase, T)> {
    if pane == DetailPane::Overlay {
        open.map(|d| (entry, d))
    } else if width == Width::Narrow {
        closing.map(|d| (OverlayPhase::Leaving, d))
    } else {
        None
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

    #[test]
    fn a_closing_overlay_is_inert() {
        let narrow = Width::Narrow;
        // The task just closed: its pane fades out and takes no input.
        let (phase, task) = overlay(
            DetailPane::Hidden,
            narrow,
            None,
            OverlayPhase::Entering,
            Some("A"),
        )
        .expect("fading out");
        assert_eq!((phase, task), (OverlayPhase::Leaving, "A"));
        assert!(!phase.interactive());
        // An open task always wins over one fading out, and takes input.
        let (phase, task) = overlay(
            DetailPane::Overlay,
            narrow,
            Some("B"),
            OverlayPhase::Entering,
            Some("A"),
        )
        .expect("open");
        assert_eq!(task, "B");
        assert!(phase.interactive());
        assert!(OverlayPhase::Resting.interactive());
        // Nothing fades over a wide window, and nothing is drawn once the fade is over.
        assert_eq!(
            overlay(
                DetailPane::Hidden,
                Width::Wide,
                None,
                OverlayPhase::Entering,
                Some("A")
            ),
            None
        );
        assert_eq!(
            overlay::<&str>(
                DetailPane::Hidden,
                narrow,
                None,
                OverlayPhase::Entering,
                None
            ),
            None
        );
    }

    #[test]
    fn a_resize_past_the_breakpoint_shows_an_open_task_at_rest() {
        // Wide with a task open, then narrowed: no entrance replay.
        assert_eq!(
            OverlayPhase::on_mount(Width::Wide, true),
            OverlayPhase::Resting
        );
        // A task opened in a narrow window slides in.
        assert_eq!(
            OverlayPhase::on_mount(Width::Narrow, false),
            OverlayPhase::Entering
        );
        // Narrowed and opened in the same frame: it opened, so it slides in.
        assert_eq!(
            OverlayPhase::on_mount(Width::Wide, false),
            OverlayPhase::Entering
        );
    }
}
