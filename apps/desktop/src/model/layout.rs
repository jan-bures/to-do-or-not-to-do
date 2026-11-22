//! How the main window arranges its panes at its width.
//!
//! At `desktop::THREE_PANE_MIN` and wider, the sidebar, the room and the detail pane sit side by side. A
//! narrower window keeps the room at its full width: the detail pane opens over the right part of the room,
//! with a scrim over the rest of it, and the room header puts its counts under the title so the title keeps
//! its full size.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::motion::Track;
use crate::theme::{desktop, elevation, motion};

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

/// A side of the room with a resize handle when that pane is docked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Detail,
}

impl Pane {
    pub const fn default_width(self) -> f32 {
        match self {
            Self::Sidebar => desktop::SIDEBAR,
            Self::Detail => desktop::DETAIL,
        }
    }

    pub const fn min(self) -> f32 {
        match self {
            Self::Sidebar => desktop::SIDEBAR_MIN,
            Self::Detail => desktop::DETAIL_MIN,
        }
    }

    const fn max(self) -> f32 {
        match self {
            Self::Sidebar => desktop::SIDEBAR_MAX,
            Self::Detail => desktop::DETAIL_MAX,
        }
    }

    /// Positive screen movement widens the left pane and narrows the right one.
    fn signed(self, delta: f32) -> f32 {
        match self {
            Self::Sidebar => delta,
            Self::Detail => -delta,
        }
    }
}

/// The device's preferred widths, before making room for the list at the current window size.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PaneWidths {
    sidebar: f32,
    detail: f32,
}

impl Default for PaneWidths {
    fn default() -> Self {
        Self {
            sidebar: Pane::Sidebar.default_width(),
            detail: Pane::Detail.default_width(),
        }
    }
}

impl PaneWidths {
    pub fn with(mut self, pane: Pane, width: f32) -> Self {
        let width = if width.is_finite() {
            width.clamp(pane.min(), pane.max())
        } else {
            pane.default_width()
        };
        match pane {
            Pane::Sidebar => self.sidebar = width,
            Pane::Detail => self.detail = width,
        }
        self
    }

    pub fn clamped(self) -> Self {
        self.with(Pane::Sidebar, self.sidebar)
            .with(Pane::Detail, self.detail)
    }

    /// On window shrink, temporarily give docked detail less space, without changing either
    /// preference. At the fixed breakpoint even the widest sidebar fits with minimum detail and list.
    /// The overlay keeps its original width; a hidden detail takes no space.
    pub fn at(self, window: f32, detail: DetailPane) -> Columns {
        let docked = detail == DetailPane::Docked;
        let budget =
            window - desktop::LIST_MIN - if docked { 2. } else { 1. } * elevation::HAIRLINE;
        let sidebar = self.sidebar.min(
            (budget - if docked { desktop::DETAIL_MIN } else { 0. }).max(desktop::SIDEBAR_MIN),
        );
        Columns {
            window,
            sidebar,
            detail: if docked {
                self.detail.min((budget - sidebar).max(desktop::DETAIL_MIN))
            } else {
                desktop::DETAIL
            },
            docked,
            budget,
        }
    }

    /// Reset the preference even if the viewport cannot currently show the full default.
    pub fn reset(
        self,
        pane: Pane,
        visible: Columns,
        now: Duration,
        reduced: bool,
    ) -> (Self, Track) {
        let prefs = self.with(pane, pane.default_width());
        let detail = if visible.docked {
            DetailPane::Docked
        } else {
            DetailPane::Hidden
        };
        let width = prefs.at(visible.window, detail).width(pane);
        let duration = if reduced {
            motion::reduced(motion::OVERLAY_IN, false)
        } else {
            motion::OVERLAY_IN
        };
        (
            prefs,
            Track::new(
                visible.width(pane),
                width,
                now,
                Duration::from_millis(u64::from(duration)),
                motion::STANDARD,
            ),
        )
    }
}

/// The widths painted this frame and the budget available to a handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    window: f32,
    sidebar: f32,
    detail: f32,
    docked: bool,
    budget: f32,
}

impl Columns {
    pub fn width(self, pane: Pane) -> f32 {
        match pane {
            Pane::Sidebar => self.sidebar,
            Pane::Detail => self.detail,
        }
    }

    pub fn max(self, pane: Pane) -> f32 {
        let other = match pane {
            Pane::Sidebar if self.docked => desktop::DETAIL_MIN,
            Pane::Sidebar => 0.,
            Pane::Detail => self.sidebar,
        };
        (self.budget - other).clamp(pane.min(), pane.max())
    }

    /// Hit strip inset from the sidebar's left edge or the detail pane's right edge. The hairline
    /// begins one pixel into either strip, leaving the scrollbar on its left fully clear.
    pub fn handle_inset(self, pane: Pane) -> f32 {
        let line = elevation::HAIRLINE;
        match pane {
            Pane::Sidebar => self.sidebar - 2. * line,
            Pane::Detail => self.detail + line - desktop::PANE_HANDLE_HIT,
        }
    }

    /// The readable room budget, excluding its side divider hairlines. An overlay takes none of it.
    pub fn room_width(self) -> f32 {
        self.window
            - self.sidebar
            - elevation::HAIRLINE
            - if self.docked {
                self.detail + elevation::HAIRLINE
            } else {
                0.
            }
    }

    pub fn step(self, pane: Pane, right: bool, shift: bool) -> f32 {
        let step = if shift {
            desktop::PANE_KEY_STEP_LARGE
        } else {
            desktop::PANE_KEY_STEP
        };
        let delta = if right { step } else { -step };
        (self.width(pane) + pane.signed(delta)).clamp(pane.min(), self.max(pane))
    }
}

/// Resolve every move relative to the original grab, not the last rendered frame. This holds the limit
/// still when a constrained detail recovers width as the sidebar shrinks, and avoids drift at clamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grab {
    pub pane: Pane,
    origin: f32,
    start: f32,
    max: f32,
}

impl Grab {
    pub fn new(pane: Pane, x: f32, columns: Columns) -> Self {
        Self {
            pane,
            origin: x,
            start: columns.width(pane),
            max: columns.max(pane),
        }
    }

    /// `None` means the displayed width is unchanged: keep the original preference, including
    /// a width temporarily capped by the viewport.
    pub fn moved(self, x: f32) -> Option<f32> {
        let width = self.width_at(x);
        (width != self.width_at(self.origin)).then_some(width)
    }

    pub fn width_at(self, x: f32) -> f32 {
        (self.start + self.pane.signed(x - self.origin))
            .round()
            .clamp(self.pane.min(), self.max.floor())
    }
}

/// Repeated arrow presses update the live width, but only the final preference needs saving.
#[derive(Default)]
pub struct KeyResize(Option<PaneWidths>);

impl KeyResize {
    pub fn step(
        &mut self,
        prefs: PaneWidths,
        columns: Columns,
        pane: Pane,
        right: bool,
        shift: bool,
    ) -> Option<PaneWidths> {
        let width = columns.step(pane, right, shift);
        if width == columns.width(pane) {
            return None;
        }
        self.0.get_or_insert(prefs);
        Some(prefs.with(pane, width))
    }

    pub fn finish(&mut self, prefs: PaneWidths) -> bool {
        self.0.take().is_some_and(|start| prefs != start)
    }
}

/// The scroll offset that brings a row (`target`, its top and bottom) into a scrolled pane's `viewport`,
/// `margin` clear of its edges, or `None` when it's in view already. Positions are in the same space
/// (the window's); `offset` is the pane's (0 at the top, more negative further down) and `max` how far
/// it can scroll. The keyboard never lands on something the owner can't see.
pub fn reveal_offset(
    viewport: (f32, f32),
    target: (f32, f32),
    offset: f32,
    max: f32,
    margin: f32,
) -> Option<f32> {
    let (top, bottom) = viewport;
    let shift = if target.0 - margin < top {
        top - (target.0 - margin)
    } else if target.1 + margin > bottom {
        bottom - (target.1 + margin)
    } else {
        return None;
    };
    let next = (offset + shift).clamp(-max.max(0.), 0.);
    (next != offset).then_some(next)
}

#[cfg(test)]
mod tests {
    #[test]
    fn focus_scrolls_what_it_lands_on_into_view() {
        use super::reveal_offset;
        // A pane from 100 to 500, scrolled to the top, 1000 px further to go.
        // Below it: scrolled up just enough to show it, with the margin.
        assert_eq!(
            reveal_offset((100., 500.), (700., 730.), 0., 1000., 8.),
            Some(-238.)
        );
        // Above it (the pane scrolled down): back down to it.
        assert_eq!(
            reveal_offset((100., 500.), (40., 70.), -300., 1000., 8.),
            Some(-232.)
        );
        // In view: nothing to do.
        assert_eq!(
            reveal_offset((100., 500.), (200., 230.), -50., 1000., 8.),
            None
        );
        // Never past either end.
        assert_eq!(
            reveal_offset((100., 500.), (2000., 2030.), -900., 1000., 8.),
            Some(-1000.)
        );
        assert_eq!(
            reveal_offset((100., 500.), (40., 70.), -20., 1000., 8.),
            Some(0.)
        );
        assert_eq!(reveal_offset((100., 500.), (40., 70.), 0., 0., 8.), None);
    }

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
        let prefs = PaneWidths::default()
            .with(Pane::Sidebar, desktop::SIDEBAR_MAX)
            .with(Pane::Detail, desktop::DETAIL_MAX);
        let columns = prefs.at(desktop::THREE_PANE_MIN, DetailPane::Docked);
        assert_eq!(columns.width(Pane::Sidebar), desktop::SIDEBAR_MAX);
        assert!(columns.width(Pane::Detail) >= desktop::DETAIL_MIN);
        assert_eq!(columns.room_width(), desktop::LIST_MIN);
        assert_eq!(
            prefs.at(1600., DetailPane::Docked).width(Pane::Detail),
            desktop::DETAIL_MAX
        );
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
        let prefs = PaneWidths::default()
            .with(Pane::Sidebar, desktop::SIDEBAR_MAX)
            .with(Pane::Detail, desktop::DETAIL_MAX);
        let full = MIN_WINDOW - desktop::SIDEBAR_MAX - elevation::HAIRLINE;
        let overlay = prefs.at(MIN_WINDOW, DetailPane::Overlay);
        assert_eq!(overlay.width(Pane::Detail), desktop::DETAIL);
        assert_eq!(overlay.room_width(), full);
        assert_eq!(prefs.at(MIN_WINDOW, DetailPane::Hidden).room_width(), full);
        assert!(full >= desktop::LIST_MIN);
        // The reference window: the list column is the design's 800 (less the hairlines).
        let wide = desktop::SIDEBAR + desktop::LIST + desktop::DETAIL;
        assert_eq!(
            PaneWidths::default()
                .at(wide, DetailPane::Docked)
                .room_width(),
            desktop::LIST - 2. * elevation::HAIRLINE
        );
    }

    #[test]
    fn drag_tracks_one_to_one_and_can_back_away_from_a_clamp() {
        let columns = PaneWidths::default().at(desktop::THREE_PANE_MIN, DetailPane::Docked);
        let side = Grab::new(Pane::Sidebar, 250., columns);
        assert_eq!(side.moved(500.), Some(columns.max(Pane::Sidebar)));
        assert_eq!(side.moved(240.), Some(desktop::SIDEBAR - 10.));
        assert_eq!(side.moved(250.), None);
        assert_eq!(side.moved(0.), Some(desktop::SIDEBAR_MIN));
        let detail = Grab::new(Pane::Detail, 808., columns);
        assert_eq!(detail.moved(820.), Some(desktop::DETAIL - 12.));
        assert_eq!(detail.moved(700.), Some(columns.max(Pane::Detail)));
        for width in [side.moved(500.), side.moved(0.)].into_iter().flatten() {
            let resized = PaneWidths::default().with(Pane::Sidebar, width);
            assert!(
                resized
                    .at(desktop::THREE_PANE_MIN, DetailPane::Docked)
                    .room_width()
                    >= desktop::LIST_MIN
            );
        }
    }

    #[test]
    fn handle_hit_strips_keep_both_scrollbar_tracks_clear() {
        let prefs = PaneWidths::default();
        let line = elevation::HAIRLINE;
        let hit = desktop::PANE_HANDLE_HIT;
        for (window, placement) in [
            (MIN_WINDOW, DetailPane::Overlay),
            (desktop::THREE_PANE_MIN, DetailPane::Docked),
            (1440.5, DetailPane::Docked),
        ] {
            for prefs in [prefs, prefs.with(Pane::Sidebar, desktop::SIDEBAR_MAX)] {
                let columns = prefs.at(window, placement);
                let sidebar_edge = columns.width(Pane::Sidebar) - line;
                let detail_edge = window - columns.width(Pane::Detail);
                for (pane, edge, start) in [
                    (
                        Pane::Sidebar,
                        sidebar_edge,
                        columns.handle_inset(Pane::Sidebar),
                    ),
                    (
                        Pane::Detail,
                        detail_edge,
                        window - columns.handle_inset(Pane::Detail) - hit,
                    ),
                ] {
                    if pane == Pane::Detail && placement != DetailPane::Docked {
                        continue;
                    }
                    let track_end = edge - desktop::SCROLLBAR_INSET;
                    let track_start = track_end - desktop::SCROLLBAR_HOVER_WIDTH;
                    assert!(start > track_end || start + hit < track_start, "{pane:?}");
                    assert_eq!(start + line, edge, "the hairline starts at the seam");
                }
            }
        }
    }

    #[test]
    fn sidebar_can_widen_when_the_list_is_already_at_its_minimum() {
        let prefs = PaneWidths::default().with(Pane::Detail, desktop::DETAIL_MAX);
        let window = desktop::THREE_PANE_MIN;
        let columns = prefs.at(window, DetailPane::Docked);
        assert_eq!(columns.room_width(), desktop::LIST_MIN);
        assert_eq!(columns.max(Pane::Sidebar), desktop::SIDEBAR_MAX);
        let step = columns.step(Pane::Sidebar, true, false);
        assert_eq!(step, desktop::SIDEBAR + desktop::PANE_KEY_STEP);
        assert_eq!(
            Grab::new(Pane::Sidebar, 0., columns).moved(desktop::PANE_KEY_STEP),
            Some(step)
        );
        let widened = prefs
            .with(Pane::Sidebar, step)
            .at(window, DetailPane::Docked);
        assert_eq!(widened.room_width(), desktop::LIST_MIN);
        assert_eq!(
            widened.width(Pane::Detail),
            columns.width(Pane::Detail) - desktop::PANE_KEY_STEP
        );
    }

    #[test]
    fn fractional_drag_positions_produce_whole_widths_even_at_a_fractional_limit() {
        let columns = PaneWidths::default().at(desktop::THREE_PANE_MIN + 0.75, DetailPane::Docked);
        let drag = Grab::new(Pane::Detail, 800.25, columns);
        assert_eq!(drag.moved(800.), None);
        assert_eq!(drag.moved(798.75), Some(desktop::DETAIL + 2.));
        assert_eq!(drag.moved(700.), Some(columns.max(Pane::Detail).floor()));

        let prefs = PaneWidths::default().with(Pane::Detail, desktop::DETAIL_MAX);
        let columns = prefs.at(desktop::THREE_PANE_MIN + 0.75, DetailPane::Docked);
        assert!(columns.width(Pane::Detail).fract() > 0.);
        assert_eq!(Grab::new(Pane::Detail, 800.25, columns).moved(800.25), None);
    }

    #[test]
    fn key_resize_writes_only_a_changed_preference_at_the_end_of_the_gesture() {
        let start = PaneWidths::default();
        let mut prefs = start;
        let mut resize = KeyResize::default();
        for right in [true, true, false] {
            prefs = resize
                .step(
                    prefs,
                    prefs.at(1440., DetailPane::Docked),
                    Pane::Sidebar,
                    right,
                    false,
                )
                .expect("each repeated arrow updates the live width");
        }
        assert_eq!(
            prefs,
            start.with(Pane::Sidebar, desktop::SIDEBAR + desktop::PANE_KEY_STEP)
        );
        assert!(resize.finish(prefs));
        assert!(
            !resize.finish(prefs),
            "a blur after key-up doesn't save twice"
        );
        let limit = start.with(Pane::Sidebar, desktop::SIDEBAR_MAX);
        assert!(
            resize
                .step(
                    limit,
                    limit.at(1440., DetailPane::Docked),
                    Pane::Sidebar,
                    true,
                    false
                )
                .is_none()
        );
        assert!(!resize.finish(limit), "an arrow at the clamp doesn't save");
    }

    #[test]
    fn keyboard_moves_the_seam_in_the_arrow_direction_and_clamps() {
        let columns = PaneWidths::default().at(1440., DetailPane::Docked);
        assert_eq!(
            columns.step(Pane::Sidebar, true, false),
            desktop::SIDEBAR + desktop::PANE_KEY_STEP
        );
        assert_eq!(
            columns.step(Pane::Detail, true, true),
            desktop::DETAIL - desktop::PANE_KEY_STEP_LARGE
        );
        assert_eq!(
            columns.step(Pane::Detail, false, false),
            desktop::DETAIL + desktop::PANE_KEY_STEP
        );
        let tight = PaneWidths::default()
            .with(Pane::Sidebar, desktop::SIDEBAR_MAX - desktop::PANE_KEY_STEP)
            .at(desktop::THREE_PANE_MIN, DetailPane::Docked);
        assert_eq!(
            tight.step(Pane::Sidebar, true, true),
            tight.max(Pane::Sidebar)
        );
        let min = PaneWidths::default()
            .with(Pane::Detail, desktop::DETAIL_MIN)
            .at(1440., DetailPane::Docked);
        assert_eq!(min.step(Pane::Detail, true, false), desktop::DETAIL_MIN);
    }

    #[test]
    fn reset_animates_from_the_visible_width_and_saves_the_default_even_when_constrained() {
        let prefs = PaneWidths::default().with(Pane::Detail, desktop::DETAIL_MAX);
        let visible = prefs.at(1440., DetailPane::Docked);
        let now = Duration::from_secs(1);
        let (reset, track) = prefs.reset(Pane::Detail, visible, now, false);
        assert_eq!(reset, PaneWidths::default());
        assert_eq!(track.at(now), visible.width(Pane::Detail));
        assert!(track.moving(now));
        assert_eq!(track.at(now + Duration::from_secs(1)), desktop::DETAIL);
        let tight = prefs.with(Pane::Sidebar, desktop::SIDEBAR_MAX);
        let (reset, track) = tight.reset(
            Pane::Detail,
            tight.at(desktop::THREE_PANE_MIN, DetailPane::Docked),
            now,
            true,
        );
        assert_eq!(
            track.at(now),
            reset
                .at(desktop::THREE_PANE_MIN, DetailPane::Docked)
                .width(Pane::Detail)
        );
        assert!(!track.moving(now));
        assert_eq!(
            reset.at(1440., DetailPane::Docked).width(Pane::Detail),
            desktop::DETAIL
        );
    }
}
