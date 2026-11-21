//! Small building blocks shared by the views. Every value comes from `theme`.

use gpui::{Div, FontWeight, Hsla, SharedString, Styled, Svg, div, prelude::*, px, svg};

use crate::fixtures::ListRef;
use crate::theme::{self, Agent, Palette, Theme, font, radius, text, weight};

/// Typography shortcuts: family, size, line height and weight in one call.
pub trait Type: Styled + Sized {
    fn ui(self, (size, line): (f32, f32), w: f32) -> Self {
        self.font_family(font::UI)
            .text_size(px(size))
            .line_height(px(line))
            .font_weight(FontWeight(w))
    }
    fn mono(self, (size, line): (f32, f32), w: f32) -> Self {
        self.font_family(font::MONO)
            .text_size(px(size))
            .line_height(px(line))
            .font_weight(FontWeight(w))
    }
    fn display(self, (size, line): (f32, f32), w: f32) -> Self {
        self.font_family(font::DISPLAY)
            .text_size(px(size))
            .line_height(px(line))
            .font_weight(FontWeight(w))
    }
}

impl<T: Styled> Type for T {}

/// A single-colour stroke icon from `assets/icons`.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A hand-drawn list glyph.
pub fn glyph(name: &str, size: f32, color: Hsla) -> Svg {
    icon(&format!("glyph-{name}"), size, color)
}

/// The round checkbox. Overdue tasks get the danger ring and wash.
pub fn checkbox(p: &Palette, overdue: bool, size: f32) -> Div {
    let (border, bg) = if overdue {
        (p.danger, p.danger_bg)
    } else {
        (p.control, gpui::transparent_black())
    };
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .border(px(1.5))
        .border_color(border)
        .bg(bg)
}

/// An agent's round letter mark.
pub fn agent_mark(theme: Theme, agent: Agent, size: f32) -> Div {
    let (bg, ink, ring) = theme::agent_colors(theme, agent);
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(ring)
        .flex()
        .items_center()
        .justify_center()
        .mono((size * 0.5, size), weight::SEMIBOLD)
        .text_color(ink)
        .child(agent.letter())
}

/// A list chip: dot plus "Collection · List" (smart lists) or just the list name.
pub fn chip(theme: Theme, list: &ListRef, with_collection: bool) -> Div {
    let c = theme::list_colors(theme, list.hue);
    let label: SharedString = if with_collection {
        format!("{} · {}", list.collection, list.name).into()
    } else {
        list.name.into()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(20.))
        .px(px(8.))
        .rounded(px(radius::CHIP))
        .bg(c.tint)
        .text_color(c.ink)
        .ui((12.0, 20.0), 550.0)
        .child(div().size(px(6.)).rounded_full().bg(c.dot))
        .child(label)
}

/// The overdue badge.
pub fn overdue_badge(p: &Palette, label: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(20.))
        .px(px(8.))
        .rounded(px(radius::CHIP))
        .bg(p.danger_bg)
        .text_color(p.danger_ink)
        .mono((12.0, 20.0), weight::SEMIBOLD)
        .child(icon("diamond", 7., p.danger))
        .child(label)
}

/// A keycap.
pub fn kbd(p: &Palette, label: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(radius::SM))
        .border_1()
        .border_b_2()
        .border_color(p.line_strong)
        .bg(p.raised)
        .text_color(p.ink_3)
        .mono((10.5, 14.0), weight::MEDIUM)
        .child(label)
}

/// `!`, `!!` or `!!!`, stepping from ink-3 to ink.
pub fn priority(p: &Palette, level: u8) -> Div {
    let (color, w, label) = match level {
        1 => (p.ink_3, weight::SEMIBOLD, "!"),
        2 => (p.ink_2, weight::BOLD, "!!"),
        3 => (p.ink, weight::BOLD, "!!!"),
        _ => (p.ink_4, weight::REGULAR, ""),
    };
    // The marks sit tight together, as in the mockup's -1.5 px tracking.
    div()
        .flex()
        .flex_none()
        .text_color(color)
        .mono((13.0, 20.0), w)
        .children(
            label
                .chars()
                .enumerate()
                .map(|(i, c)| div().when(i > 0, |d| d.ml(px(-2.5))).child(c.to_string())),
        )
}

/// (bars, lit bars) for step progress: one bar per step, at most five, lit in proportion.
pub fn meter_bars(done: u32, total: u32) -> (u32, u32) {
    let bars = total.clamp(1, 5);
    let lit = if total == 0 {
        0
    } else {
        (done.min(total) * bars).div_ceil(total)
    };
    (bars, lit)
}

/// Step progress: one bar per step (up to five), the done ones in the accent.
pub fn meter(p: &Palette, done: u32, total: u32) -> Div {
    let (bars, lit) = meter_bars(done, total);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .children((0..bars).map(|i| {
            div().w(px(3.)).h(px(10.)).rounded(px(1.)).bg(if i < lit {
                p.accent
            } else {
                p.line_strong
            })
        }))
}

/// The small mono capitals used for section labels.
pub fn label(p: &Palette, s: &'static str) -> Div {
    div()
        .text_color(p.ink_3)
        .mono(text::LABEL, weight::SEMIBOLD)
        .child(s.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::meter_bars;

    #[test]
    fn meter_lights_bars_in_proportion() {
        assert_eq!(meter_bars(2, 5), (5, 2));
        assert_eq!(meter_bars(0, 4), (4, 0));
        assert_eq!(meter_bars(1, 3), (3, 1));
        assert_eq!(meter_bars(5, 10), (5, 3));
        assert_eq!(meter_bars(10, 10), (5, 5));
        assert_eq!(meter_bars(12, 10), (5, 5));
        assert_eq!(meter_bars(0, 0), (1, 0));
    }
}
