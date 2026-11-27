//! Small building blocks shared by the views. Every value comes from `theme` (that is, `todo_design`).

mod caption;
mod scrollbar;

pub use caption::{CaptionPreview, caption_button_tints, window_controls};
pub use scrollbar::{SCROLLBAR_GUTTER, ScrollPane, Scrollbar};

use gpui::{
    Div, Hsla, IntoElement, SharedString, Stateful, Styled, Svg, Window, div, prelude::*, px, svg,
};

use crate::assets::{glyph_path, icon_path};
use crate::model::ListRef;
use crate::theme::{
    self, Agent, Glyph, Icon, Palette, TextStyled, Theme, component, desktop, elevation, radius,
    space, ty,
};

/// Interaction styling shared by enabled borderless action buttons.
pub trait QuietButton {
    fn quiet_button(self, palette: Palette) -> Self;
}

impl QuietButton for Stateful<Div> {
    fn quiet_button(self, palette: Palette) -> Self {
        self.cursor_pointer()
            .hover(move |s| s.bg(palette.button_hover))
            .active(move |s| s.bg(palette.button_pressed))
    }
}

/// The hit target of a toolbar icon button.
pub fn toolbar_icon() -> Div {
    div()
        .size(px(component::ICON_BUTTON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::MD))
}

/// A Lucide icon, tinted like text. `size` is `component::ICON` (18) in toolbars and rows and
/// `component::ICON_SMALL` (14) in chips and metadata.
pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .path(icon_path(icon))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The active ink shared by task toggles and their menu equivalents.
pub fn task_toggle_fill(p: &Palette, glyph: Icon) -> Option<Hsla> {
    match glyph {
        Icon::SunFilled => Some(p.button_sun_ink),
        Icon::StarFilled => Some(p.button_important_fill),
        _ => None,
    }
}

/// A binary glyph: quiet outline off, semantic fill on. The fill may fade and pop without resizing
/// the hit target or displacing the outline.
pub fn task_toggle(p: &Palette, glyph: Icon, size: f32, fill: f32, scale: f32) -> Div {
    let filled = match glyph {
        Icon::Sun => Icon::SunFilled,
        _ => Icon::StarFilled,
    };
    let color = task_toggle_fill(p, filled).unwrap_or(p.ink_3);
    let scaled = size * scale;
    let inset = px((size - scaled) / 2.);
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .when(fill < 1., |d| {
            d.child(icon(glyph, size, theme::with_alpha(p.ink_3, 1. - fill)))
        })
        .when(fill > 0., |d| {
            d.child(
                icon(filled, scaled, theme::with_alpha(color, fill))
                    .absolute()
                    .left(inset)
                    .top(inset),
            )
        })
}

/// A hand-drawn list glyph at `component::GLYPH`.
pub fn glyph(glyph: Glyph, color: Hsla) -> Svg {
    glyph_at(glyph, color, component::GLYPH)
}

pub fn glyph_at(glyph: Glyph, color: Hsla, size: f32) -> Svg {
    svg()
        .path(glyph_path(glyph))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A live status light centered in one text line. In a wrapping row, top-align the row.
pub fn status_light(color: Hsla, style: ty::TextStyle) -> Div {
    div()
        .flex_none()
        .h(px(style.line_height))
        .flex()
        .items_center()
        .child(
            div()
                .size(px(component::STATUS_LIGHT))
                .rounded_full()
                .bg(color),
        )
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
        .border(px(component::CHECKBOX_RING))
        .border_color(border)
        .bg(bg)
}

/// The tick's size against its checkbox.
const TICK: f32 = 0.75;

/// A checked checkbox. On a selected (`wash`) row it uses `check_selected`, which keeps 3:1 there.
pub fn checked(p: &Palette, on_wash: bool, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(if on_wash { p.check_selected } else { p.accent })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(Icon::Check, size * TICK, p.on_accent))
}

/// A checkbox on its way between empty and checked (see `model::motion::RowLook`): the disc at `fill`
/// opacity and `scale`, over the empty ring until it covers it, and the tick drawn `tick` of the way from
/// the left. At `(1, 1, 1)` it is exactly [`checked`], and at `fill` 0 exactly [`checkbox`].
pub fn checkbox_at(
    p: &Palette,
    overdue: bool,
    on_wash: bool,
    size: f32,
    (fill, scale, tick): (f32, f32, f32),
) -> Div {
    let covered = fill >= 1. && scale >= 1.;
    let disc = size * scale;
    let tick_size = size * TICK;
    let inset = |s: f32| px((size - s) / 2.);
    let color = if on_wash { p.check_selected } else { p.accent };
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .when(!covered, |d| {
            d.child(checkbox(p, overdue, size).absolute().top_0().left_0())
        })
        .when(fill > 0., |d| {
            d.child(
                div()
                    .absolute()
                    .left(inset(disc))
                    .top(inset(disc))
                    .size(px(disc))
                    .rounded_full()
                    .bg(theme::with_alpha(color, color.a * fill)),
            )
            .child(
                div()
                    .absolute()
                    .left(inset(tick_size))
                    .top(inset(tick_size))
                    .h(px(tick_size))
                    .w(px(tick_size * tick))
                    .overflow_hidden()
                    .child(icon(
                        Icon::Check,
                        tick_size,
                        theme::with_alpha(p.on_accent, p.on_accent.a * fill),
                    )),
            )
        })
}

/// An agent's round letter mark: the letter at half the mark's size, with a 1 px inset ring.
pub fn agent_mark(theme: Theme, agent: Agent, size: f32) -> Div {
    let c = theme::agent_colors(theme, agent);
    let letter = ty::AGENT_LETTER;
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(c.bg)
        .border(px(elevation::HAIRLINE))
        .border_color(c.ring)
        .flex()
        .items_center()
        .justify_center()
        .style(letter)
        .text_size(px(size * letter.size / component::AGENT_MARK))
        .text_color(c.ink)
        .child(agent.letter())
}

/// The shared shape of the list and overdue pills.
fn pill(bg: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(component::CHIP))
        .px(px(component::CHIP_PAD_X))
        .rounded(px(radius::CHIP))
        .bg(bg)
}

/// What a list chip says: "Collection · List" (smart lists) or just the list name.
pub fn chip_label(list: &ListRef, with_collection: bool) -> SharedString {
    todo_client::task_labels::list_chip(
        &list.name,
        list.collection.as_deref().filter(|_| with_collection),
    )
    .into()
}

/// A tinted list pill with [`chip_label`]. It is at most `desktop::CHIP_MAX` wide and gives way when its row
/// is short of room; a label that doesn't fit ends in an ellipsis inside the pill (laid out short, never
/// clipped).
pub fn chip(theme: Theme, list: &ListRef, with_collection: bool) -> Div {
    let c = theme::list_colors(theme, list.hue);
    pill(c.tint)
        .flex_shrink(1.)
        .min_w_0()
        .max_w(px(desktop::CHIP_MAX))
        .text_color(c.ink)
        .style(ty::CHIP)
        .child(
            div()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(chip_label(list, with_collection)),
        )
}

/// How wide [`chip`] draws: its measured label plus padding, up to `desktop::CHIP_MAX`.
pub fn chip_width(window: &Window, list: &ListRef, with_collection: bool) -> f32 {
    let label = text_width(window, ty::CHIP, &chip_label(list, with_collection));
    (2. * component::CHIP_PAD_X + label).min(desktop::CHIP_MAX)
}

/// The narrowest [`chip`] gets in a metadata line: its padding and an ellipsis.
pub fn chip_min_width(window: &Window) -> f32 {
    2. * component::CHIP_PAD_X + text_width(window, ty::CHIP, "…")
}

/// The width `text` takes on one line in `style`, as GPUI lays it out (`TextStyled::style` sets the same
/// family, size, weight and slant). Names reach here through `labels::one_line` already; a stray line
/// break is still measured as a space, since one-line shaping can't take one.
pub fn text_width(window: &Window, style: ty::TextStyle, text: &str) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let text = crate::model::labels::one_line(text);
    let font = gpui::Font {
        weight: gpui::FontWeight(f32::from(style.weight)),
        style: if style.italic {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        },
        ..gpui::font(style.family)
    };
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line =
        window
            .text_system()
            .shape_line(SharedString::from(text), px(style.size), &[run], None);
    f32::from(line.width)
}

/// The overdue badge.
pub fn overdue_badge(p: &Palette, label: SharedString) -> Div {
    pill(p.danger_bg)
        .text_color(p.danger_ink)
        .style(ty::OVERDUE_BADGE)
        .child(label)
}

/// How wide a keycap (`kbd`) is: its label, its padding and its side borders.
pub fn kbd_width(window: &Window, label: &str) -> f32 {
    text_width(window, ty::KEYCAP, label) + 2. * (KBD_PAD + elevation::HAIRLINE)
}

const KBD_PAD: f32 = space::S1 + 1.;

/// A keycap.
pub fn kbd(p: &Palette, label: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(KBD_PAD))
        .h(px(ty::KEYCAP.line_height + 2.))
        .flex()
        .items_center()
        .rounded(px(radius::SM))
        .border(px(elevation::HAIRLINE))
        .border_b_2()
        .border_color(p.line_strong)
        .bg(p.raised)
        .text_color(p.ink_3)
        .style(ty::KEYCAP)
        .child(label)
}

/// `!`, `!!` or `!!!`, stepping from ink-3 to ink. GPUI has no letter spacing, so the marks are laid out
/// one by one with the style's (negative) tracking.
pub fn priority(p: &Palette, level: u8) -> Div {
    let (color, style, label) = match level {
        1 => (p.ink_3, ty::PRIORITY.with_weight(600), "!"),
        2 => (p.ink_2, ty::PRIORITY, "!!"),
        3 => (p.ink, ty::PRIORITY, "!!!"),
        _ => (p.ink_4, ty::PRIORITY, ""),
    };
    let tracking = style.tracking_px();
    div()
        .flex()
        .flex_none()
        .text_color(color)
        .style(style)
        .children(label.chars().enumerate().map(|(i, c)| {
            div()
                .when(i > 0, |d| d.ml(px(tracking)))
                .child(c.to_string())
        }))
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

/// How wide [`meter`] draws for `total` steps.
pub fn meter_width(total: u32) -> f32 {
    let (bars, _) = meter_bars(0, total);
    let bars = bars as f32;
    bars * elevation::SELECTION_BAR + (bars - 1.) * space::S1 / 2.
}

/// Step progress: one bar per step (up to five), the done ones in the accent. Each bar is a 3 × 10 tick
/// with a 2 px gap, as in the design.
pub fn meter(p: &Palette, done: u32, total: u32) -> Div {
    let (bars, lit) = meter_bars(done, total);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::S1 / 2.))
        .children((0..bars).map(|i| {
            div()
                .w(px(elevation::SELECTION_BAR))
                .h(px(component::ICON_SMALL - space::S1))
                .rounded(px(elevation::HAIRLINE))
                .bg(if i < lit { p.accent } else { p.line_strong })
        }))
}

/// A transient overlay (menus, popovers): raised, with the design's one shadow, and a 1 px border in
/// dark mode where shadows barely read.
pub fn overlay(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .py(px(space::S1))
        .rounded(px(radius::LG))
        .bg(p.raised)
        .border(px(elevation::HAIRLINE))
        .border_color(if p.night() { p.line_strong } else { p.line })
        .shadow(overlay_shadow(p))
}

/// A full-width footer painted inside the dialog border. GPUI's overflow clip is rectangular,
/// so the footer fill itself needs the dialog's inner bottom radii.
pub fn dialog_footer(p: &Palette, corner_radius: f32) -> Div {
    let inner = px(corner_radius - elevation::HAIRLINE);
    div()
        .flex_none()
        .bg(p.canvas)
        .border_t(px(elevation::HAIRLINE))
        .border_color(p.line)
        .rounded_bl(inner)
        .rounded_br(inner)
}

/// Leading mark over a positioned surface. The inset and rounded ends keep it clear of the outline.
pub fn leading_rail(color: Hsla, width: f32) -> Div {
    div()
        .absolute()
        .left(px(space::S1))
        .top(px(space::S1))
        .bottom(px(space::S1))
        .w(px(width))
        .rounded_full()
        .bg(color)
}

/// The design's one shadow, for transient overlays: menus, popovers and the narrow window's detail.
pub fn overlay_shadow(p: &Palette) -> Vec<gpui::BoxShadow> {
    let layers = if p.night() {
        elevation::OVERLAY_DARK
    } else {
        elevation::OVERLAY_LIGHT
    };
    layers
        .iter()
        .map(|s| gpui::BoxShadow {
            color: theme::with_alpha(theme::color(s.color.rgb), s.color.alpha),
            offset: gpui::point(px(0.), px(s.y)),
            blur_radius: px(s.blur),
            spread_radius: px(s.spread),
            inset: false,
        })
        .collect()
}

/// A primary button: accent fill, label in `on_accent`.
pub fn button(p: &Palette, label: impl Into<SharedString>) -> Div {
    div()
        .h(px(component::BUTTON))
        .px(px(space::S4))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(radius::BUTTON))
        .bg(p.accent_fill)
        .text_color(p.on_accent)
        .style(ty::BUTTON)
        .child(label.into())
}

/// A quiet button: a hairline outline, label in ink.
pub fn button_quiet(p: &Palette, label: impl Into<SharedString>) -> Div {
    div()
        .h(px(component::BUTTON))
        .px(px(space::S4))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(radius::BUTTON))
        .border(px(elevation::HAIRLINE))
        .border_color(p.line_strong)
        .text_color(p.ink_2)
        .style(ty::BUTTON)
        .child(label.into())
}

/// A section label in mono capitals.
pub fn label(p: &Palette, s: &'static str) -> Div {
    div()
        .text_color(p.ink_3)
        .style(ty::CAPS_LABEL)
        .child(s.to_uppercase())
}

/// A hover tooltip (`.tooltip(kit::tooltip(p, "…"))`): one quiet line on the overlay surface.
pub fn tooltip(
    p: Palette,
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| Tooltip {
            p,
            text: text.clone(),
        })
        .into()
    }
}

struct Tooltip {
    p: Palette,
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        overlay(&self.p)
            .px(px(space::S2 + 2.))
            .style(ty::TAG)
            .text_color(self.p.ink_2)
            .child(self.text.clone())
    }
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
